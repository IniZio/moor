(() => {
  window.rowsBusy = () => document.querySelector('#mrows details[open], #mrows .confirm') !== null;
  const copy = (button) => {
    const text = document.getElementById(button.dataset.copy).textContent;
    const done = () => { button.textContent = 'Copied'; };
    if (navigator.clipboard) {
      navigator.clipboard.writeText(text).then(done);
      return;
    }
    const range = document.createRange();
    range.selectNodeContents(document.getElementById(button.dataset.copy));
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    button.textContent = 'Selected';
  };
  document.addEventListener('click', (e) => {
    const button = e.target.closest('[data-copy]');
    if (button) {
      copy(button);
      return;
    }
    document.querySelectorAll('details.menu[open]').forEach((d) => {
      if (!d.contains(e.target)) d.open = false;
    });
  });
})();
