// The page's only native bridge. Injected at document start into the top frame
// of every page the game shell loads.
//
// The API is deliberately small: a page can ask to play an id from the library,
// leave, rescan, favourite, or reveal a folder. It cannot name a command, a
// path or an argument - those live in the Rust process.
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
    version: "1",
    // A game can opt out if Select+Start is a move in the game itself.
    exitCombo: true,
    play: (id) => send({ cmd: "play", id: String(id) }),
    quit: () => send({ cmd: "quit" }),
    back: () => send({ cmd: "back" }),
    rescan: () => send({ cmd: "rescan" }),
    favorite: (id, on) => send({ cmd: "favorite", id: String(id), value: !!on }),
    reveal: (id) => send({ cmd: "reveal", id: String(id) }),
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

  // Ctrl+Q is the keyboard way out of a game; the library also has Q and
  // Escape. Capture it before the game's own handlers see it.
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
