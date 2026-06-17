//! Lo — pure-native Rust voice agent. Entry point + threading model.
//!
//! The winit event loop owns the main thread (it is intentionally NOT async). A
//! multi-thread tokio runtime hosts the worker (brain loop, backends, tools,
//! downloads). On-device ML (whisper ASR, Silero VAD) runs on a dedicated
//! "listen" std thread that owns the !Send models; Kokoro TTS runs on its own
//! std thread (spawned by the worker). cpal audio streams (also !Send) live on
//! the main thread inside the `App`.
//!
//! Bridges (replacing Electron IPC, see `events`):
//!   - UI/listen → worker: `mpsc::UnboundedSender<UiCommand>`
//!   - worker/ML → UI: `EventLoopProxy<AppEvent>` → `ApplicationHandler::user_event`
//!   - barge-in epoch: a `watch::channel<u64>` the UI bumps and the worker/TTS
//!     observe to abandon stale work.

// Don't pop a console window on Windows release builds.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
// The binary carries APIs that are wired up in later phases (wake-word
// activation, settings hot-reload, engine prewarm, the status-HUD dot). Allow
// dead_code crate-wide so CI's `-D warnings` stays green without prematurely
// deleting that scaffolding; `lo-core` (the tested core) stays strictly clean.
#![allow(dead_code)]

mod app;
mod audio;
mod backends;
mod brain;
mod events;
mod gui;
mod listen;
mod ml;
mod tools;
mod worker;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use lo_core::LoSettings;
use tracing_subscriber::EnvFilter;
use winit::event_loop::EventLoop;

use crate::app::App;
use crate::events::{AppEvent, UiCommand};

fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("LO_LOG").unwrap_or_else(|_| EnvFilter::new("info,lo=debug")),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut settings = LoSettings::load();
    // Right-size the brain model to this machine when the user left it on
    // `auto`/the shipped default — otherwise a 16 GB box would try to load a 30B
    // model. An explicit model in settings.json is always honored verbatim.
    {
        let kind = lo_core::backends::resolve_backend_kind(&settings);
        let resolved =
            lo_core::backends::resolve_model_id(&settings.model, kind, detect_ram_bytes());
        if resolved != settings.model {
            tracing::info!(from = %settings.model, to = %resolved, "auto-selected brain model for this machine");
            settings.model = resolved;
        }
    }
    tracing::info!(backend = ?settings.backend, model = %settings.model, "lo starting");

    // Headless self-check: verify subsystems initialize without opening a window.
    if std::env::args().any(|a| a == "--smoke") {
        return smoke(&settings);
    }

    // Headless end-to-end: run ONE agent turn for the given text and exit (prints
    // the streamed reply + any tool calls). No window, no audio.
    if let Some(text) = arg_value("--turn") {
        return run_turn_headless(&settings, &text);
    }

    // cpal audio: the !Send AudioEngine stays on this (main) thread inside App;
    // the Send AudioHandle is shared with the worker + listen thread.
    let (mut audio_engine, audio_handle) = audio::new().context("audio init failed")?;
    // LO_NO_MIC skips opening the audio streams entirely — used to launch the GUI
    // on macOS without tripping the microphone TCC gate (which kills an un-bundled
    // binary). The orb still renders; the AudioHandle just reports zero level.
    if std::env::var("LO_NO_MIC").is_ok() {
        tracing::warn!("LO_NO_MIC set — audio streams disabled (no capture/playback)");
    } else if let Err(e) = audio_engine.start() {
        // Non-fatal: the UI still runs (e.g. no mic granted yet).
        tracing::warn!("audio start failed: {e:#}");
    }

    // Channels / shared state.
    let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiCommand>();
    let (epoch_tx, epoch_rx) = tokio::sync::watch::channel::<u64>(0);
    let ptt_active = Arc::new(AtomicBool::new(false));
    // Set on exit so the listen thread leaves its loop and can be joined.
    let shutdown = Arc::new(AtomicBool::new(false));

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("failed to build event loop")?;
    let proxy = event_loop.create_proxy();

    // Worker on a dedicated thread with a current-thread tokio runtime. The
    // worker's futures touch !Send values (tool execution, recursive directory
    // walks), so they can't live on a shared multi-thread pool; a current-thread
    // runtime's `block_on` imposes no `Send` bound.
    let worker_handle = {
        let ctx = worker::WorkerCtx {
            ui_rx,
            proxy: proxy.clone(),
            settings: settings.clone(),
            audio: audio_handle.clone(),
            epoch_rx: epoch_rx.clone(),
        };
        std::thread::Builder::new()
            .name("lo-worker".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build worker runtime");
                rt.block_on(worker::run(ctx));
            })
            .expect("failed to spawn worker thread")
    };

    // Listen thread (std): owns ASR/VAD/wake, drains capture, emits transcripts.
    let listen_handle = listen::spawn(listen::ListenCtx {
        audio: audio_handle.clone(),
        proxy: proxy.clone(),
        settings: settings.clone(),
        ptt_active: ptt_active.clone(),
        shutdown: shutdown.clone(),
    });

    let mut app = App::new(app::AppCtx {
        audio_engine,
        audio: audio_handle,
        ui_tx,
        epoch_tx,
        ptt_active,
        settings,
        // `--say "<text>"` injects a synthetic transcript once the window is up,
        // driving a full turn (brain → captions → TTS) for visual/E2E testing.
        pending_say: arg_value("--say"),
    });

    let run_result = event_loop.run_app(&mut app);

    // Graceful shutdown. Dropping `app` releases `ui_tx` (the worker's recv then
    // returns None and it stops the managed engine) and the cpal `AudioEngine`
    // (capture/playback streams stop). The flag releases the listen loop. Both
    // joins are bounded so a thread stuck in a first-run download can't wedge exit.
    shutdown.store(true, Ordering::SeqCst);
    drop(app);
    join_timeout(listen_handle, Duration::from_secs(2), "listen");
    join_timeout(worker_handle, Duration::from_secs(4), "worker");

    run_result.context("event loop error")?;
    Ok(())
}

/// Total system memory in bytes (via `sysinfo`), for the model RAM ladder.
fn detect_ram_bytes() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// Join `handle`, but give up after `dur` so shutdown can't hang forever (e.g. a
/// thread blocked mid first-run model download). The process exits either way.
fn join_timeout(handle: std::thread::JoinHandle<()>, dur: Duration, name: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        let _ = tx.send(());
    });
    if rx.recv_timeout(dur).is_err() {
        tracing::warn!("{name} thread did not exit within {dur:?}; exiting anyway");
    }
}

/// `lo --smoke`: verify every subsystem initializes without opening a window
/// (settings, backend endpoint resolution, the tool registry, audio devices, and
/// which ML engines were compiled in). Used for headless / CI sanity checks.
fn smoke(settings: &LoSettings) -> anyhow::Result<()> {
    println!("lo smoke check");
    println!(
        "  config file: {}",
        lo_core::config::paths::settings_file().display()
    );
    println!(
        "  settings: backend={:?} model={} voice={} activation={:?}",
        settings.backend, settings.model, settings.voice, settings.activation_mode
    );
    let ep = lo_core::backends::resolve_endpoint(settings);
    println!(
        "  backend: kind={:?} url={} model={}",
        ep.kind, ep.base_url, ep.model_id
    );
    let ram = lo_core::backends::models::total_ram_gb(detect_ram_bytes());
    let tier = lo_core::backends::models::recommend_tier(ram);
    println!(
        "  hardware: {ram:.0} GB RAM → recommended tier {} (min {} GB)",
        tier.label, tier.min_ram_gb
    );
    println!(
        "  tools: {} registered",
        lo_core::tools::tool_schemas().len()
    );
    match audio::new() {
        Ok(_) => println!("  audio: default input+output devices OK"),
        Err(e) => println!("  audio: unavailable ({e})"),
    }
    println!(
        "  ml: asr-whisper={} tts-kokoro={} vad-silero={} wake-openwakeword={}",
        cfg!(feature = "asr-whisper"),
        cfg!(feature = "tts-kokoro"),
        cfg!(feature = "vad-silero"),
        cfg!(feature = "wake-openwakeword"),
    );
    println!("OK");
    Ok(())
}

/// Value of a `--flag value` or `--flag=value` CLI argument, if present.
fn arg_value(flag: &str) -> Option<String> {
    let mut args = std::env::args();
    let eq = format!("{flag}=");
    while let Some(a) = args.next() {
        if a == flag {
            return args.next();
        }
        if let Some(v) = a.strip_prefix(&eq) {
            return Some(v.to_string());
        }
    }
    None
}

/// `lo --turn "<text>"`: run a single agent turn headlessly against the configured
/// backend, streaming the reply to stdout and printing any tool calls + results.
/// Exercises the full brain pipeline (HTTP/SSE → tool dispatch → final reply)
/// without a window or audio.
fn run_turn_headless(settings: &LoSettings, text: &str) -> anyhow::Result<()> {
    use lo_core::brain::{
        append_tool_round, build_request_body, initial_convo_with_time, Sampling, MAX_ROUNDS,
    };
    use lo_core::types::{ChatMessage, ChatRole};
    use std::io::Write as _;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let engine = backends::Engine::new();
        engine
            .ensure_ready(settings, None)
            .await
            .context("engine not ready")?;
        let endpoint = engine.endpoint(settings);
        eprintln!(
            "[turn] backend={:?} url={}",
            endpoint.kind, endpoint.base_url
        );

        let history = vec![ChatMessage {
            role: ChatRole::User,
            content: text.to_string(),
        }];
        let now = tools::datetime_context();
        let mut convo = initial_convo_with_time(settings, &history, Some(&now));
        let (announce_tx, _announce_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut final_text = String::new();
        let sampling = Sampling::from_settings(settings);

        for _round in 0..MAX_ROUNDS {
            let body = build_request_body(&endpoint.model_id, &convo, sampling);
            let mut on_delta = |d: &str| {
                print!("{d}");
                let _ = std::io::stdout().flush();
            };
            let (t, calls) = brain::stream_completion(&endpoint, body, &mut on_delta).await?;
            if calls.is_empty() {
                final_text = t.trim().to_string();
                break;
            }
            println!();
            let mut results = Vec::new();
            for c in &calls {
                println!("[tool] {}({})", c.function.name, c.function.arguments);
                let r = tools::dispatch(
                    &c.function.name,
                    &c.function.arguments,
                    settings,
                    &announce_tx,
                )
                .await;
                println!("[tool result] {r}");
                results.push(r);
            }
            append_tool_round(&mut convo, &t, calls, results);
        }

        println!("\n[final reply] {final_text}");
        engine.stop();
        Ok::<(), anyhow::Error>(())
    })
}
