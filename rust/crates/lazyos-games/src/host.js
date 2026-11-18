// The game's only native bridge. Injected at document start into the top frame
// of the page the host loads.
//
// A game gets two things: a way to leave (the host returns to the TUI library),
// and the Select+Start / Ctrl+Q combination that does the same. The library and
// the launch decisions live in the Rust process, not here.
(() => {
  "use strict";

  const send = (message) => {
    try {
      window.webkit.messageHandlers.lazyos.postMessage(message);
    } catch (error) {
      console.error("LazyOS bridge is unavailable:", error);
    }
  };

  const api = {
    version: "2",
    // A game can opt out if Select+Start is a move in the game itself.
    exitCombo: true,
    quit: () => send({ cmd: "quit" }),
    back: () => send({ cmd: "back" }),
  };

  Object.defineProperty(window, "LazyOS", {
    value: api,
    writable: false,
    configurable: false,
  });

  // Select+Start (buttons 8 and 9), held for 600 ms. Polled with the same
  // frame clock the games use; nothing runs when the page is hidden.
  let held = 0;
  const poll = () => {
    if (api.exitCombo && navigator.getGamepads) {
      let down = false;
      for (const pad of navigator.getGamepads()) {
        if (!pad) continue;
        const buttons = pad.buttons;
        if (buttons && buttons[8] && buttons[9] && buttons[8].pressed && buttons[9].pressed) {
          down = true;
          break;
        }
      }
      held = down ? held + 16 : 0;
      if (held >= 600) {
        held = 0;
        api.quit();
        return;
      }
    }
    requestAnimationFrame(poll);
  };
  requestAnimationFrame(poll);

  // Ctrl+Q is the keyboard way out; Escape belongs to the game.
  addEventListener(
    "keydown",
    (event) => {
      if (event.ctrlKey && !event.altKey && !event.metaKey && (event.key === "q" || event.key === "Q")) {
        event.preventDefault();
        event.stopPropagation();
        api.quit();
      }
    },
    true,
  );
})();
