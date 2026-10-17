// Apply the saved theme before the stylesheet loads to avoid a flash on reload.
(() => {
  const storageKey = "yaml-rt-theme";
  const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
  let preference = "system";
  try {
    const saved = localStorage.getItem(storageKey);
    if (["system", "light", "dark"].includes(saved)) preference = saved;
  } catch {
    // Theme selection still works when browser storage is unavailable.
  }

  function applyTheme() {
    document.documentElement.dataset.theme = preference === "system"
      ? (systemTheme.matches ? "dark" : "light")
      : preference;
  }

  applyTheme();
  systemTheme.addEventListener("change", applyTheme);
  document.addEventListener("DOMContentLoaded", () => {
    const selector = document.getElementById("theme");
    selector.value = preference;
    selector.addEventListener("change", () => {
      preference = selector.value;
      applyTheme();
      try {
        localStorage.setItem(storageKey, preference);
      } catch {
        // Keep the selected theme for this visit even without persistence.
      }
    });
  });
})();
