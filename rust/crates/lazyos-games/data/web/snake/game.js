// Snake: a complete little game, and the template for the next one.
//
// The host gives it: the `lazyos-game://` origin (so localStorage is private to
// this game), the Gamepad API, the session palette as CSS variables, and the
// Select+Start / Ctrl+Q exit. Everything else is here.
"use strict";

const COLS = 32;
const ROWS = 20;
const CELL = 24;
const STEP_MS = 110;

const canvas = document.getElementById("game");
const ctx = canvas.getContext("2d");
const scoreEl = document.getElementById("score");
const bestEl = document.getElementById("best");
const overlay = document.getElementById("overlay");
const overlayTitle = document.getElementById("overlay-title");
const overlayText = document.getElementById("overlay-text");
const overlayStart = document.getElementById("overlay-start");

let colors = palette();
let best = readBest();
bestEl.textContent = String(best);

const state = {
  snake: [],
  direction: { x: 1, y: 0 },
  next: { x: 1, y: 0 },
  food: { x: 0, y: 0 },
  score: 0,
  phase: "ready", // ready | playing | paused | dead
};

function palette() {
  const style = getComputedStyle(document.documentElement);
  const read = (name, fallback) => style.getPropertyValue(name).trim() || fallback;
  return {
    bg: read("--bg", "#002b36"),
    surface: read("--surface", "#073642"),
    border: read("--border", "#15505c"),
    accent: read("--accent", "#268bd2"),
    accentAlt: read("--accent-alt", "#2aa198"),
    warn: read("--warn", "#b58900"),
    strong: read("--strong", "#93a1a1"),
    dim: read("--dim", "#657b83"),
  };
}

function readBest() {
  try {
    return Number(localStorage.getItem("snake.best") || "0") || 0;
  } catch (_) {
    return 0;
  }
}

function writeBest(value) {
  try {
    localStorage.setItem("snake.best", String(value));
  } catch (_) {
    /* a game still works without a high score */
  }
}

function reset() {
  state.snake = [
    { x: 8, y: 10 },
    { x: 7, y: 10 },
    { x: 6, y: 10 },
  ];
  state.direction = { x: 1, y: 0 };
  state.next = { x: 1, y: 0 };
  state.score = 0;
  state.phase = "ready";
  state.food = randomFood();
  scoreEl.textContent = "0";
}

function randomFood() {
  const free = [];
  for (let y = 0; y < ROWS; y += 1) {
    for (let x = 0; x < COLS; x += 1) {
      if (!state.snake.some((part) => part.x === x && part.y === y)) {
        free.push({ x, y });
      }
    }
  }
  return free[Math.floor(Math.random() * free.length)] || { x: 0, y: 0 };
}

function start() {
  reset();
  state.phase = "playing";
  overlay.classList.add("hidden");
  last = performance.now();
  accumulator = 0;
}

function pause() {
  if (state.phase === "playing") {
    state.phase = "paused";
    showOverlay("Paused", "P or Start to continue", "");
  } else if (state.phase === "paused") {
    state.phase = "playing";
    overlay.classList.add("hidden");
    last = performance.now();
  }
}

function showOverlay(title, text, start) {
  overlayTitle.textContent = title;
  overlayText.textContent = text;
  overlayStart.textContent = start;
  overlay.classList.remove("hidden");
}

function die() {
  state.phase = "dead";
  if (state.score > best) {
    best = state.score;
    bestEl.textContent = String(best);
    writeBest(best);
  }
  showOverlay("Game over", `Score ${state.score} · Best ${best}`, "Enter or A to restart");
}

function step() {
  const next = state.next;
  if (next.x !== -state.direction.x || next.y !== -state.direction.y) {
    state.direction = next;
  }
  const head = {
    x: state.snake[0].x + state.direction.x,
    y: state.snake[0].y + state.direction.y,
  };
  const eating = head.x === state.food.x && head.y === state.food.y;
  const body = eating ? state.snake : state.snake.slice(0, -1);
  if (
    head.x < 0 ||
    head.x >= COLS ||
    head.y < 0 ||
    head.y >= ROWS ||
    body.some((part) => part.x === head.x && part.y === head.y)
  ) {
    die();
    return;
  }
  state.snake.unshift(head);
  if (eating) {
    state.score += 1;
    scoreEl.textContent = String(state.score);
    state.food = randomFood();
  } else {
    state.snake.pop();
  }
}

function rounded(x, y, size, radius) {
  ctx.beginPath();
  if (ctx.roundRect) {
    ctx.roundRect(x, y, size, size, radius);
  } else {
    ctx.rect(x, y, size, size);
  }
  ctx.fill();
}

function draw() {
  ctx.fillStyle = colors.bg;
  ctx.fillRect(0, 0, canvas.width, canvas.height);

  ctx.strokeStyle = colors.surface;
  ctx.lineWidth = 1;
  for (let x = 1; x < COLS; x += 1) {
    ctx.beginPath();
    ctx.moveTo(x * CELL + 0.5, 0);
    ctx.lineTo(x * CELL + 0.5, canvas.height);
    ctx.stroke();
  }
  for (let y = 1; y < ROWS; y += 1) {
    ctx.beginPath();
    ctx.moveTo(0, y * CELL + 0.5);
    ctx.lineTo(canvas.width, y * CELL + 0.5);
    ctx.stroke();
  }

  ctx.fillStyle = colors.warn;
  ctx.beginPath();
  ctx.arc(
    state.food.x * CELL + CELL / 2,
    state.food.y * CELL + CELL / 2,
    CELL * 0.32,
    0,
    Math.PI * 2,
  );
  ctx.fill();

  state.snake.forEach((part, index) => {
    ctx.fillStyle = index === 0 ? colors.accentAlt : colors.accent;
    rounded(part.x * CELL + 3, part.y * CELL + 3, CELL - 6, 5);
  });

  if (state.phase === "ready") {
    ctx.fillStyle = colors.dim;
    ctx.font = "14px 'JetBrains Mono', monospace";
    ctx.textAlign = "center";
    ctx.fillText("Enter or A to start", canvas.width / 2, canvas.height - 18);
  }
}

let last = performance.now();
let accumulator = 0;

function loop(now) {
  if (state.phase === "playing") {
    accumulator += now - last;
    while (accumulator >= STEP_MS) {
      accumulator -= STEP_MS;
      step();
      if (state.phase !== "playing") break;
    }
  }
  last = now;
  draw();
  pollPad();
  requestAnimationFrame(loop);
}

function turn(x, y) {
  if (state.phase === "ready") {
    start();
  }
  state.next = { x, y };
}

function key(event) {
  const key = event.key;
  if (key === "ArrowLeft" || key === "a" || key === "A") turn(-1, 0);
  else if (key === "ArrowRight" || key === "d" || key === "D") turn(1, 0);
  else if (key === "ArrowUp" || key === "w" || key === "W") turn(0, -1);
  else if (key === "ArrowDown" || key === "s" || key === "S") turn(0, 1);
  else if (key === "p" || key === "P") pause();
  else if (key === "Enter" || key === " ") {
    if (state.phase === "dead" || state.phase === "ready") start();
    else if (state.phase === "paused") pause();
  } else {
    return;
  }
  event.preventDefault();
}

addEventListener("keydown", key);

// Gamepad: D-pad or left stick to turn, A to start/restart, Start to pause.
let padHeld = {};
let padA = false;
let padStart = false;

function pollPad() {
  const pads = navigator.getGamepads ? navigator.getGamepads() : [];
  const pad = Array.from(pads).find(Boolean);
  if (!pad) return;
  const axisX = pad.axes[0] || 0;
  const axisY = pad.axes[1] || 0;
  const pressed = (index) => !!pad.buttons[index]?.pressed;
  const direction = {
    left: pressed(14) || axisX < -0.55,
    right: pressed(15) || axisX > 0.55,
    up: pressed(12) || axisY < -0.55,
    down: pressed(13) || axisY > 0.55,
  };
  for (const [name, down] of Object.entries(direction)) {
    if (down && !padHeld[name]) {
      const step = { left: [-1, 0], right: [1, 0], up: [0, -1], down: [0, 1] }[name];
      turn(step[0], step[1]);
    }
    padHeld[name] = down;
  }
  const a = pressed(0);
  if (a && !padA && (state.phase === "ready" || state.phase === "dead")) start();
  padA = a;
  const start = pressed(9);
  if (start && !padStart) pause();
  padStart = start;
}

// Touch: a swipe is a turn. The board is the gesture surface, not the page.
let touchStart = null;
canvas.addEventListener(
  "touchstart",
  (event) => {
    const touch = event.changedTouches[0];
    touchStart = { x: touch.clientX, y: touch.clientY };
  },
  { passive: true },
);
canvas.addEventListener(
  "touchend",
  (event) => {
    if (!touchStart) return;
    const touch = event.changedTouches[0];
    const dx = touch.clientX - touchStart.x;
    const dy = touch.clientY - touchStart.y;
    touchStart = null;
    if (Math.abs(dx) < 18 && Math.abs(dy) < 18) {
      if (state.phase === "ready" || state.phase === "dead") start();
      return;
    }
    if (Math.abs(dx) > Math.abs(dy)) turn(dx > 0 ? 1 : -1, 0);
    else turn(0, dy > 0 ? 1 : -1);
  },
  { passive: true },
);

addEventListener("visibilitychange", () => {
  if (document.hidden && state.phase === "playing") pause();
});

reset();
showOverlay("Snake", "Arrows or D-pad to move · P to pause", "Press Enter or A");
requestAnimationFrame(loop);
