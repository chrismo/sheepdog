// Loaded in each page's <head>, so a pick is on <html> before the first paint.
(() => {
  const PICKS = [
    ["pal", "Colors", [["drystone", "Drystone"], ["fleece", "Fleece & Raddle"], ["heather", "Heather Moor"], ["lamp", "Lamplight"]]],
    ["face", "Face", [["londrina", "Londrina Solid"], ["alfa", "Alfa Slab One"], ["shoulders", "Big Shoulders Display"], ["familjen", "Familjen Grotesk"]]],
    ["theme", "Light or dark", [["", "as the system is"], ["light", "light"], ["dark", "dark"]]],
  ];
  const root = document.documentElement;
  const read = k => { try { return localStorage.getItem("sd-" + k); } catch { return null; } };
  const save = (k, v) => { try { localStorage.setItem("sd-" + k, v); } catch {} };
  const apply = (k, v, options) => {
    if (!options.some(([id]) => id === v)) return;
    if (v) root.dataset[k] = v; else delete root.dataset[k];
  };
  for (const [k, , options] of PICKS) {
    const v = read(k);
    if (v !== null) apply(k, v, options);
  }
  // sd view restarts itself on a new build (#496): reload for its pages and
  // its token; the hash keeps the page's place.
  let build = null;
  const look = () => fetch("/api/build").then(r => r.json()).then(b => {
    if (build !== null && b.build !== build) location.reload();
    build = b.build;
  }).catch(() => {});
  look();
  setInterval(look, 5000);
  const esc = s => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
  addEventListener("DOMContentLoaded", () => {
    const spot = document.getElementById("look");
    if (!spot) return;
    const el = document.createElement("details");
    el.id = "look";
    el.className = "look";
    el.innerHTML = `<summary title="colors, face, light or dark">Look</summary><div class="menu">${PICKS.map(([k, name, options]) => {
      const now = root.dataset[k] || "";
      return `<label>${name}<select data-look="${k}">${options.map(([id, label]) =>
        `<option value="${id}"${id === now ? " selected" : ""}>${esc(label)}</option>`).join("")}</select></label>`;
    }).join("")}</div>`;
    spot.replaceWith(el);
    document.addEventListener("click", e => { if (!el.contains(e.target)) el.open = false; });
    // Capture, so an Escape that closes the menu doesn't reach the page's own
    // Escape handler (which closes the open sheet).
    document.addEventListener("keydown", e => {
      if (e.key !== "Escape" || !el.open) return;
      e.stopPropagation();
      el.open = false;
      el.querySelector("summary").focus();
    }, true);
    el.addEventListener("change", e => {
      const k = e.target.dataset.look, pick = PICKS.find(p => p[0] === k);
      if (!pick) return;
      apply(k, e.target.value, pick[2]);
      save(k, e.target.value);
    });
  });
})();
