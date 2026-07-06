// Theme-aware mermaid initialization for the vendored bundle.
// Pages that contain a mermaid fence load mermaid.min.js and then this
// file; the generator emits diagrams as <pre class="mermaid"> blocks,
// which startOnLoad picks up.
(function () {
  var dark =
    window.matchMedia &&
    window.matchMedia("(prefers-color-scheme: dark)").matches;
  mermaid.initialize({
    startOnLoad: true,
    theme: dark ? "dark" : "neutral",
  });
})();
