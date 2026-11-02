// The pages' GitHub-data line (#401): the data's age, "asking GitHub…" while
// a fetch runs, and the refresh button. Each page loads it before its own script.
document.head.insertAdjacentHTML("beforeend", `<style>
.ghline { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; }
.refresh { font: inherit; font-size: 0.78rem; color: var(--ink); background: none; border: 1px solid var(--line); border-radius: 6px; padding: 0.1rem 0.5rem; cursor: pointer; }
</style>`);
const TOKEN = document.querySelector('meta[name="sd-token"]').content;
const ago = iso => {
  const s = Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.round(s / 60)}m` : `${Math.round(s / 3600)}h`;
};
const ghLine = d => `<p class="none ghline">${d.fetched ? `GitHub data ${ago(d.fetched)} old` : d.error ? "No answer from GitHub yet" : "Not asked GitHub yet"}` +
  `${d.refreshing ? " · asking GitHub…" : ""} <button class="refresh" type="button" data-refresh>refresh</button></p>`;
async function refreshGh() {
  const r = await fetch("/api/refresh", { method: "POST", headers: { "X-Sd-Token": TOKEN } });
  if (!r.ok) {
    document.querySelectorAll("[data-refresh]").forEach(b => (b.textContent = `refused (${r.status}): reload the page`));
    throw new Error(r.status);
  }
}
