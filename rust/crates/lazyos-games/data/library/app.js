// The library page. Everything it knows comes from library.json, which the
// Rust process generates; everything it does goes through window.LazyOS, which
// only accepts an id. No path, no command line, no shell.
"use strict";

const STORE = "lazyos.games.view";
const COARSE = matchMedia("(pointer: coarse)").matches;

const state = {
  data: null,
  filter: "all",
  query: "",
  selected: 0,
  games: [],
};

const el = {
  app: document.getElementById("app"),
  platforms: document.getElementById("platforms"),
  grid: document.getElementById("grid"),
  empty: document.getElementById("empty"),
  notice: document.getElementById("notice"),
  title: document.getElementById("title"),
  subtitle: document.getElementById("subtitle"),
  count: document.getElementById("count"),
  search: document.getElementById("search"),
  scan: document.getElementById("scan"),
  quit: document.getElementById("quit"),
  toast: document.getElementById("toast"),
};

function restore() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORE) || "{}");
    if (typeof saved.filter === "string") state.filter = saved.filter;
    if (Number.isInteger(saved.index)) state.selected = saved.index;
  } catch (_) {
    /* a corrupt view is not worth a message */
  }
}

function saveView() {
  try {
    localStorage.setItem(
      STORE,
      JSON.stringify({ filter: state.filter, index: state.selected }),
    );
  } catch (_) {
    /* private storage is optional */
  }
}

async function load() {
  try {
    const response = await fetch("library.json", { cache: "no-store" });
    if (!response.ok) throw new Error(`library.json: ${response.status}`);
    state.data = await response.json();
  } catch (error) {
    state.data = { version: 1, roots: [], retroarch: false, platforms: [], games: [] };
    toast(String(error));
  }
  if (!state.data.platforms.some((platform) => platform.id === state.filter)) {
    state.filter = "all";
  }
  apply();
}

function platformTitle(id) {
  const platform = state.data.platforms.find((candidate) => candidate.id === id);
  return platform ? platform.title : id;
}

function filtered() {
  let games = state.data.games.slice();
  if (state.filter === "recent") {
    games = games.filter((game) => game.last > 0);
    games.sort((a, b) => b.last - a.last);
  } else if (state.filter === "favorites") {
    games = games.filter((game) => game.favorite);
  } else if (state.filter !== "all") {
    games = games.filter((game) => game.platform === state.filter);
  }
  const query = state.query.trim().toLowerCase();
  if (query) {
    games = games.filter((game) =>
      `${game.title} ${game.platform_title}`.toLowerCase().includes(query),
    );
  }
  return games;
}

function apply() {
  const previous = state.games[state.selected]?.id;
  state.games = filtered();
  if (previous) {
    const index = state.games.findIndex((game) => game.id === previous);
    if (index >= 0) state.selected = index;
  }
  state.selected = Math.max(0, Math.min(state.selected, state.games.length - 1));
  renderRail();
  renderHeader();
  renderNotice();
  renderGrid();
  saveView();
}

function renderRail() {
  el.platforms.replaceChildren();
  for (const platform of state.data.platforms) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "rail-item";
    button.setAttribute("aria-current", String(platform.id === state.filter));
    button.addEventListener("click", () => {
      state.filter = platform.id;
      state.selected = 0;
      apply();
      el.grid.focus();
    });
    const title = document.createElement("span");
    title.textContent = platform.title;
    const count = document.createElement("span");
    count.className = "n";
    count.textContent = String(platform.count);
    button.append(title, count);
    el.platforms.append(button);
  }
}

function renderHeader() {
  el.title.textContent = platformTitle(state.filter);
  const roots = state.data.roots || [];
  el.subtitle.textContent = roots.length ? roots.join("  ·  ") : "";
  el.count.textContent =
    state.games.length === 1 ? "1 game" : `${state.games.length} games`;
}

function renderNotice() {
  const retro = state.data.games.some((game) => game.kind === "retro");
  if (retro && !state.data.retroarch) {
    el.notice.hidden = false;
    el.notice.textContent =
      "RetroArch is not installed. Retro games are listed but cannot start; see 08-games in Learn.";
  } else {
    el.notice.hidden = true;
  }
}

function initials(game) {
  const words = (game.platform_title || game.title).split(/\s+/).filter(Boolean);
  return words
    .map((word) => word[0])
    .join("")
    .slice(0, 3)
    .toUpperCase();
}

function renderGrid() {
  el.grid.replaceChildren();
  el.empty.hidden = state.games.length > 0;
  el.grid.hidden = state.games.length === 0;
  if (state.games.length === 0) {
    renderEmpty();
    return;
  }
  state.games.forEach((game, index) => {
    const card = document.createElement("div");
    card.className = "card";
    card.setAttribute("role", "option");
    card.setAttribute("aria-selected", String(index === state.selected));
    card.dataset.index = String(index);
    if (index === state.selected) card.classList.add("selected");

    const cover = document.createElement("div");
    cover.className = "cover";
    if (game.art) {
      const image = document.createElement("img");
      image.loading = "lazy";
      image.alt = "";
      image.src = game.art;
      image.addEventListener("error", () => {
        image.remove();
        cover.append(placeholder(game));
      });
      cover.append(image);
    } else {
      cover.append(placeholder(game));
    }

    const name = document.createElement("div");
    name.className = "name";
    name.textContent = game.title;
    name.title = game.title;

    const meta = document.createElement("div");
    meta.className = "meta";
    const platform = document.createElement("span");
    platform.textContent = game.kind === "web" ? "LazyOS" : game.platform_title;
    platform.title = game.platform_title;
    const badges = document.createElement("span");
    badges.className = "badges";
    if (game.favorite) badges.append(badge("★", "star"));
    if (game.save) badges.append(badge("save", "save"));
    if (game.kind === "retro" && !game.core_present) badges.append(badge("core", "missing"));
    meta.append(platform, badges);

    card.append(cover, name, meta);
    card.addEventListener("click", () => {
      select(index);
      if (COARSE) play(game);
    });
    card.addEventListener("dblclick", () => play(game));
    el.grid.append(card);
  });
  select(state.selected, false);
}

function placeholder(game) {
  const span = document.createElement("span");
  span.className = "initial";
  span.textContent = initials(game);
  return span;
}

function badge(text, kind) {
  const span = document.createElement("span");
  span.className = `badge ${kind}`;
  span.textContent = text;
  return span;
}

function renderEmpty() {
  el.empty.replaceChildren();
  const title = document.createElement("h2");
  title.textContent = state.query ? "Nothing matches" : "No games yet";
  const line = document.createElement("p");
  if (state.query) {
    line.textContent = `No title or platform contains “${state.query}”.`;
  } else {
    line.textContent =
      "Put ROMs in a platform folder under ~/Games (for example ~/Games/snes/Super Mario World.sfc), then Rescan.";
  }
  const roots = document.createElement("p");
  roots.textContent = `Scanning: ${(state.data.roots || []).join(", ") || "~/Games"}`;
  const web = document.createElement("p");
  web.append("LazyOS games live in ");
  const code = document.createElement("code");
  code.textContent = "~/.local/share/lazyos/games/web/<id>/game.toml";
  web.append(code, " (or /usr/share/lazyos/games/web).");
  el.empty.append(title, line, roots, web);
}

function select(index, scroll = true) {
  if (state.games.length === 0) return;
  state.selected = Math.max(0, Math.min(index, state.games.length - 1));
  const cards = el.grid.children;
  for (let i = 0; i < cards.length; i += 1) {
    const selected = i === state.selected;
    cards[i].classList.toggle("selected", selected);
    cards[i].setAttribute("aria-selected", String(selected));
  }
  if (scroll) {
    cards[state.selected]?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
  saveView();
}

function columns() {
  const value = getComputedStyle(el.grid).gridTemplateColumns;
  return value && value !== "none" ? value.split(" ").filter(Boolean).length : 1;
}

function move(dx, dy) {
  if (state.games.length === 0) return;
  const width = Math.max(1, columns());
  const next = state.selected + dx + dy * width;
  select(Math.max(0, Math.min(next, state.games.length - 1)));
}

function play(game) {
  if (!game) return;
  if (game.kind === "retro") {
    if (!game.core_present) {
      toast(`Core not installed: ${game.core || game.platform}`);
      return;
    }
    if (!state.data.retroarch) {
      toast("RetroArch is not installed");
      return;
    }
  }
  saveView();
  window.LazyOS.play(game.id);
}

function favourite(game) {
  if (!game) return;
  game.favorite = !game.favorite;
  const favorites = state.data.platforms.find((platform) => platform.id === "favorites");
  if (favorites) {
    favorites.count = state.data.games.filter((candidate) => candidate.favorite).length;
  }
  window.LazyOS.favorite(game.id, game.favorite);
  apply();
}

let toastTimer = 0;
function toast(message) {
  el.toast.textContent = message;
  el.toast.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.toast.classList.remove("show"), 2600);
}

el.search.addEventListener("input", () => {
  state.query = el.search.value;
  state.selected = 0;
  apply();
});

el.search.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    event.preventDefault();
    el.search.value = "";
    state.query = "";
    apply();
    el.search.blur();
    el.grid.focus();
  } else if (event.key === "Enter") {
    event.preventDefault();
    play(state.games[state.selected]);
  } else if (event.key === "ArrowDown") {
    event.preventDefault();
    el.search.blur();
    el.grid.focus();
  }
});

el.scan.addEventListener("click", () => {
  toast("Rescanning…");
  window.LazyOS.rescan();
});

el.quit.addEventListener("click", () => window.LazyOS.quit());

addEventListener("keydown", (event) => {
  if (document.activeElement === el.search) return;
  const key = event.key;
  if (key === "ArrowRight") {
    event.preventDefault();
    move(1, 0);
  } else if (key === "ArrowLeft") {
    event.preventDefault();
    move(-1, 0);
  } else if (key === "ArrowDown") {
    event.preventDefault();
    move(0, 1);
  } else if (key === "ArrowUp") {
    event.preventDefault();
    move(0, -1);
  } else if (key === "Home") {
    event.preventDefault();
    select(0);
  } else if (key === "End") {
    event.preventDefault();
    select(state.games.length - 1);
  } else if (key === "Enter" || key === " ") {
    event.preventDefault();
    play(state.games[state.selected]);
  } else if (key === "f" || key === "F") {
    event.preventDefault();
    favourite(state.games[state.selected]);
  } else if (key === "s" || key === "S") {
    event.preventDefault();
    toast("Rescanning…");
    window.LazyOS.rescan();
  } else if (key === "q" || key === "Q") {
    event.preventDefault();
    window.LazyOS.quit();
  } else if (key === "/") {
    event.preventDefault();
    el.search.focus();
    el.search.select();
  } else if (key === "Escape") {
    if (state.query) {
      el.search.value = "";
      state.query = "";
      apply();
    } else {
      window.LazyOS.quit();
    }
  }
});

// Gamepad navigation. The browser polls the pad; a 170 ms repeat is slow
// enough to pick one card per press and fast enough to cross a grid.
let padDirection = "";
let padNext = 0;
function pollGamepad(now) {
  const pads = navigator.getGamepads ? navigator.getGamepads() : [];
  const pad = Array.from(pads).find(Boolean);
  if (pad) {
    const axisX = pad.axes[0] || 0;
    const axisY = pad.axes[1] || 0;
    const pressed = (index) => !!pad.buttons[index]?.pressed;
    let direction = "";
    if (pressed(14) || axisX < -0.55) direction = "left";
    else if (pressed(15) || axisX > 0.55) direction = "right";
    else if (pressed(12) || axisY < -0.55) direction = "up";
    else if (pressed(13) || axisY > 0.55) direction = "down";

    if (direction !== padDirection) {
      padDirection = direction;
      padNext = now + 260;
      if (direction) {
        const step = { left: [-1, 0], right: [1, 0], up: [0, -1], down: [0, 1] }[direction];
        move(step[0], step[1]);
      }
    } else if (direction && now >= padNext) {
      padNext = now + 170;
      const step = { left: [-1, 0], right: [1, 0], up: [0, -1], down: [0, 1] }[direction];
      move(step[0], step[1]);
    }

    if (pressed(0) || pressed(9)) {
      if (!pollGamepad.aHeld) play(state.games[state.selected]);
      pollGamepad.aHeld = true;
    } else {
      pollGamepad.aHeld = false;
    }
    if (pressed(1)) {
      if (!pollGamepad.bHeld && state.query) {
        el.search.value = "";
        state.query = "";
        apply();
      }
      pollGamepad.bHeld = true;
    } else {
      pollGamepad.bHeld = false;
    }
  }
  requestAnimationFrame(pollGamepad);
}

restore();
load();
el.grid.focus();
requestAnimationFrame(pollGamepad);
