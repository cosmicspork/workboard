// Applies a saved theme before first paint. Kept outside the bundle because the CSP forbids inline scripts.
try {
  var theme = localStorage.getItem('workboard.theme');
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
} catch (_) {}
