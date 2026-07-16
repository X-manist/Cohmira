(function initializeTheme() {
  var mode = '';
  try {
    var stored = window.localStorage.getItem('cohmira.themeMode');
    if (stored === 'light' || stored === 'dark') {
      mode = stored;
    }
  } catch (_) {
    // Storage can be unavailable in hardened WebViews; the OS preference still works.
  }
  if (!mode) {
    mode = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light';
  }
  document.documentElement.setAttribute('data-theme', mode);
  document.documentElement.classList.toggle('dark', mode === 'dark');
}());
