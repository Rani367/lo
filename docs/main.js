/* ===========================================================================
   Page interactions: the hero "conversation" that drives the orb through Lo's
   real states, a scroll-linked veil + reveal system, and a center-of-viewport
   observer that hands the orb's state to whichever section you're reading.
   =========================================================================== */
(function () {
  "use strict";

  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var orb = window.LoOrb || { setState: function () {} };

  /* ---------- footer year ---------- */
  var yearEl = document.getElementById("year");
  if (yearEl) yearEl.textContent = String(new Date().getFullYear());

  /* ---------- header + veil on scroll ---------- */
  var header = document.getElementById("siteHeader");
  var root = document.documentElement;
  var ticking = false;

  function applyScroll() {
    ticking = false;
    var y = window.scrollY || window.pageYOffset || 0;
    var vh = window.innerHeight || 1;
    if (header) header.classList.toggle("scrolled", y > 40);
    var p = Math.min(1, Math.max(0, (y - vh * 0.12) / (vh * 0.55)));
    root.style.setProperty("--veil", (p * 0.92).toFixed(3));
  }
  function onScroll() {
    if (!ticking) {
      ticking = true;
      window.requestAnimationFrame(applyScroll);
    }
  }
  window.addEventListener("scroll", onScroll, { passive: true });
  window.addEventListener("resize", applyScroll);
  applyScroll();

  /* ---------- reveal on scroll (with a light per-group stagger) ---------- */
  var revealEls = document.querySelectorAll("[data-reveal]");
  ["feature-grid", "platforms", "backends", "tier-grid"].forEach(function (cls) {
    var groups = document.getElementsByClassName(cls);
    for (var g = 0; g < groups.length; g++) {
      var kids = groups[g].querySelectorAll("[data-reveal]");
      for (var i = 0; i < kids.length; i++) {
        kids[i].style.transitionDelay = (i * 0.07).toFixed(2) + "s";
      }
    }
  });

  if ("IntersectionObserver" in window && !reduceMotion) {
    var revealObs = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) {
          e.target.classList.add("in");
          revealObs.unobserve(e.target);
        }
      });
    }, { rootMargin: "0px 0px -10% 0px", threshold: 0.12 });
    revealEls.forEach(function (el) { revealObs.observe(el); });
  } else {
    revealEls.forEach(function (el) { el.classList.add("in"); });
  }

  /* ---------- which section owns the orb's state ---------- */
  var hero = document.getElementById("top");
  var heroActive = true;

  if ("IntersectionObserver" in window) {
    var stateObs = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (!e.isIntersecting) return;
        if (e.target === hero) {
          heroActive = true; // the hero conversation loop takes over
        } else {
          heroActive = false;
          orb.setState(e.target.getAttribute("data-state") || "idle");
        }
      });
    }, { rootMargin: "-45% 0px -45% 0px", threshold: 0 });
    document.querySelectorAll("[data-state]").forEach(function (el) {
      stateObs.observe(el);
    });
  }

  /* ---------- the hero conversation ---------- */
  var capYou = document.getElementById("capYou");
  var capLo = document.getElementById("capLo");

  var TURNS = [
    { you: "what's the weather and start a 10-minute timer", lo: "Clear and 68°. Timer's running — ten minutes." },
    { you: "find the rust 1.85 release notes", lo: "Found them. Opening the top result now." },
    { you: "take a screenshot and save it to my desktop", lo: "Done — it's on your desktop." },
    { you: "what's eating my disk?", lo: "Downloads, about 42 gigs. Want me to open it?" },
    { you: "play something and nudge the volume up", lo: "Playing. Volume's at sixty percent." },
  ];

  function setLine(el, text) {
    if (!el) return;
    el.textContent = text || "";
    el.classList.toggle("is-on", !!text);
  }
  function sleep(ms) {
    return new Promise(function (res) { setTimeout(res, ms); });
  }
  function maybeState(name) {
    if (heroActive) orb.setState(name);
  }
  function caret(el, kind) {
    if (!el) return;
    el.classList.remove("typing", "blink");
    if (kind) el.classList.add(kind);
  }
  // Type text in one character at a time — the first character lands at once,
  // then the rest follow with a little rhythm around punctuation.
  async function typeLine(el, text, perChar) {
    if (!el) return;
    el.classList.add("is-on");
    caret(el, "typing");
    el.textContent = "";
    for (var i = 0; i < text.length; i++) {
      el.textContent = text.slice(0, i + 1);
      var ch = text.charAt(i);
      var d = perChar + Math.random() * perChar * 0.6;
      if (ch === "." || ch === "?" || ch === "!") d += 260;
      else if (ch === "," || ch === "—" || ch === ";" || ch === ":") d += 130;
      await sleep(d);
    }
  }

  if (reduceMotion) {
    // Static: show one exchange, no typing.
    setLine(capYou, TURNS[0].you);
    setLine(capLo, TURNS[0].lo);
  } else {
    (function runConversation() {
      var i = 0;
      async function loop() {
        while (true) {
          var turn = TURNS[i % TURNS.length];
          i++;

          // listening — the request transcribes as you "speak"
          maybeState("listening");
          if (capLo) {
            capLo.classList.remove("is-on", "blink", "typing");
            capLo.textContent = "";
          }
          await typeLine(capYou, turn.you, 34);
          caret(capYou, null);

          // a quick beat of thought — kept short so the reply starts right after
          maybeState("thinking");
          await sleep(360);

          // speaking — Lo's reply streams in
          maybeState("speaking");
          await typeLine(capLo, turn.lo, 27);
          caret(capLo, "blink");
          await sleep(1400);

          // settle
          if (capYou) capYou.classList.remove("is-on");
          if (capLo) capLo.classList.remove("is-on");
          caret(capLo, null);
          maybeState("idle");
          await sleep(400);
        }
      }
      loop();
    })();
  }
})();
