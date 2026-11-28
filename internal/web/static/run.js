(() => {
  const open = new Set();
  document.addEventListener('toggle', (e) => {
    const step = e.target;
    if (step instanceof HTMLDetailsElement && step.id) {
      if (step.open) open.add(step.id);
      else open.delete(step.id);
    }
  }, true);
  document.addEventListener('htmx:afterSwap', () => {
    open.forEach((id) => {
      const step = document.getElementById(id);
      if (step && !step.open) step.open = true;
    });
  });
})();
