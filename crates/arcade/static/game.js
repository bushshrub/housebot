'use strict';

// Both scripts share the page's top-level scope, so the machine classes stay
// behind their namespace rather than being destructured into it.
const MACHINE_TYPES = window.ARCADE_MACHINES;

const ROOM_X = 18;
const ROOM_Z = 13;
const ROOM_Y = 6;
const EYE_HEIGHT = 1.65;
const WALK_SPEED = 5.2;
const PLAYER_RADIUS = 0.42;
const REACH = 3.2;

const canvas = document.getElementById('view');
const gl = canvas.getContext('webgl2', { antialias: true, alpha: false });
if (!gl) {
  document.body.innerHTML = '<div class="screen show"><h1>No WebGL 2</h1></div>';
  throw new Error('webgl2 unavailable');
}

const SOLID_VERT = `#version 300 es
in vec3 a_pos;
in vec3 a_nrm;
uniform mat4 u_viewProj;
uniform mat4 u_model;
out vec3 v_nrm;
out vec3 v_world;
void main() {
  vec4 world = u_model * vec4(a_pos, 1.0);
  v_world = world.xyz;
  v_nrm = mat3(u_model) * a_nrm;
  gl_Position = u_viewProj * world;
}`;

const SOLID_FRAG = `#version 300 es
precision highp float;
in vec3 v_nrm;
in vec3 v_world;
uniform vec3 u_color;
uniform vec3 u_emissive;
uniform vec3 u_eye;
uniform float u_carpet;
out vec4 fragColor;

const vec3 FOG = vec3(0.035, 0.028, 0.062);

// Two neon tubes down the length of the hall, so the room has somewhere for
// its light to come from.
float lamps(vec3 p) {
  float a = 1.0 / (1.0 + 0.012 * dot(p - vec3(-8.0, 5.6, 0.0), p - vec3(-8.0, 5.6, 0.0)));
  float b = 1.0 / (1.0 + 0.012 * dot(p - vec3(8.0, 5.6, 0.0), p - vec3(8.0, 5.6, 0.0)));
  return a + b;
}

void main() {
  vec3 base = u_color;
  if (u_carpet > 0.5) {
    vec2 cell = v_world.xz * 0.5;
    vec2 edge = abs(fract(cell + 0.5) - 0.5) / max(fwidth(cell), vec2(0.0001));
    float line = 1.0 - clamp(min(edge.x, edge.y), 0.0, 1.0);
    float sparkle = fract(sin(dot(floor(v_world.xz * 2.0), vec2(12.99, 78.23))) * 43758.5);
    base = mix(base, vec3(0.55, 0.12, 0.42), line * 0.7);
    base += vec3(0.10, 0.06, 0.16) * step(0.94, sparkle);
  }
  vec3 n = normalize(v_nrm);
  float ambient = 0.55;
  float lit = ambient + 2.2 * lamps(v_world) * max(dot(n, normalize(vec3(0.2, 1.0, 0.1))), 0.35);
  vec3 color = base * lit + u_emissive;
  color = mix(color, FOG, clamp(length(v_world - u_eye) / 110.0, 0.0, 1.0));
  fragColor = vec4(color, 1.0);
}`;

const SCREEN_VERT = `#version 300 es
in vec3 a_pos;
in vec3 a_nrm;
in vec2 a_uv;
uniform mat4 u_viewProj;
uniform mat4 u_model;
out vec2 v_uv;
void main() {
  v_uv = a_uv;
  gl_Position = u_viewProj * u_model * vec4(a_pos, 1.0);
}`;

const SCREEN_FRAG = `#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_screen;
uniform float u_glow;
out vec4 fragColor;
void main() {
  vec3 color = texture(u_screen, v_uv).rgb;
  // Scanlines and a vignette, so a flat quad reads as a CRT.
  float scan = 0.86 + 0.14 * sin(v_uv.y * 620.0);
  vec2 offset = v_uv - 0.5;
  float vignette = 1.0 - 0.55 * dot(offset, offset);
  fragColor = vec4(color * scan * vignette * u_glow, 1.0);
}`;

function compile(type, source) {
  const shader = gl.createShader(type);
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(gl.getShaderInfoLog(shader));
  }
  return shader;
}

function link(vertexSource, fragmentSource, attributes) {
  const program = gl.createProgram();
  gl.attachShader(program, compile(gl.VERTEX_SHADER, vertexSource));
  gl.attachShader(program, compile(gl.FRAGMENT_SHADER, fragmentSource));
  attributes.forEach((name, index) => gl.bindAttribLocation(program, index, name));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(gl.getProgramInfoLog(program));
  }
  return program;
}

const solid = link(SOLID_VERT, SOLID_FRAG, ['a_pos', 'a_nrm']);
const screen = link(SCREEN_VERT, SCREEN_FRAG, ['a_pos', 'a_nrm', 'a_uv']);

const solidUniforms = {
  viewProj: gl.getUniformLocation(solid, 'u_viewProj'),
  model: gl.getUniformLocation(solid, 'u_model'),
  color: gl.getUniformLocation(solid, 'u_color'),
  emissive: gl.getUniformLocation(solid, 'u_emissive'),
  eye: gl.getUniformLocation(solid, 'u_eye'),
  carpet: gl.getUniformLocation(solid, 'u_carpet'),
};
const screenUniforms = {
  viewProj: gl.getUniformLocation(screen, 'u_viewProj'),
  model: gl.getUniformLocation(screen, 'u_model'),
  sampler: gl.getUniformLocation(screen, 'u_screen'),
  glow: gl.getUniformLocation(screen, 'u_glow'),
};

function mesh(attributes, indices) {
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);
  attributes.forEach(({ data, size }, location) => {
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(data), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(location);
    gl.vertexAttribPointer(location, size, gl.FLOAT, false, 0, 0);
  });
  gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint16Array(indices), gl.STATIC_DRAW);
  gl.bindVertexArray(null);
  return { vao, count: indices.length };
}

const CUBE_FACES = [
  { n: [0, 0, 1], u: [1, 0, 0], v: [0, 1, 0] },
  { n: [0, 0, -1], u: [-1, 0, 0], v: [0, 1, 0] },
  { n: [1, 0, 0], u: [0, 0, -1], v: [0, 1, 0] },
  { n: [-1, 0, 0], u: [0, 0, 1], v: [0, 1, 0] },
  { n: [0, 1, 0], u: [1, 0, 0], v: [0, 0, -1] },
  { n: [0, -1, 0], u: [1, 0, 0], v: [0, 0, 1] },
];

function buildCube() {
  const positions = [], normals = [], indices = [];
  CUBE_FACES.forEach((face, i) => {
    for (const [su, sv] of [[-1, -1], [1, -1], [1, 1], [-1, 1]]) {
      for (let axis = 0; axis < 3; axis++) {
        positions.push((face.n[axis] + su * face.u[axis] + sv * face.v[axis]) * 0.5);
      }
      normals.push(...face.n);
    }
    const base = i * 4;
    indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
  });
  return mesh([{ data: positions, size: 3 }, { data: normals, size: 3 }], indices);
}

const cube = buildCube();
const panel = mesh(
  [
    { data: [-0.5, -0.5, 0, 0.5, -0.5, 0, 0.5, 0.5, 0, -0.5, 0.5, 0], size: 3 },
    { data: [0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1], size: 3 },
    { data: [0, 1, 1, 1, 1, 0, 0, 0], size: 2 },
  ],
  [0, 1, 2, 0, 2, 3],
);

function m4mul(a, b) {
  const out = new Float32Array(16);
  for (let c = 0; c < 4; c++) {
    for (let r = 0; r < 4; r++) {
      let sum = 0;
      for (let k = 0; k < 4; k++) sum += a[k * 4 + r] * b[c * 4 + k];
      out[c * 4 + r] = sum;
    }
  }
  return out;
}

function m4perspective(fovy, aspect, near, far) {
  const f = 1 / Math.tan(fovy / 2);
  return new Float32Array([
    f / aspect, 0, 0, 0,
    0, f, 0, 0,
    0, 0, (far + near) / (near - far), -1,
    0, 0, (2 * far * near) / (near - far), 0,
  ]);
}

function m4view(eye, right, up, forward) {
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  return new Float32Array([
    right[0], up[0], -forward[0], 0,
    right[1], up[1], -forward[1], 0,
    right[2], up[2], -forward[2], 0,
    -dot(right, eye), -dot(up, eye), dot(forward, eye), 1,
  ]);
}

function m4box(position, scale, yaw, pitch = 0) {
  const cy = Math.cos(yaw), sy = Math.sin(yaw);
  const cp = Math.cos(pitch), sp = Math.sin(pitch);
  const right = [cy, 0, -sy];
  const up = [sy * sp, cp, cy * sp];
  const forward = [sy * cp, -sp, cy * cp];
  return new Float32Array([
    right[0] * scale[0], right[1] * scale[0], right[2] * scale[0], 0,
    up[0] * scale[1], up[1] * scale[1], up[2] * scale[1], 0,
    forward[0] * scale[2], forward[1] * scale[2], forward[2] * scale[2], 0,
    position[0], position[1], position[2], 1,
  ]);
}

function normalize(v) {
  const length = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / length, v[1] / length, v[2] / length];
}

function cross(a, b) {
  return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
}

function drawSolid(geometry, model, color, emissive, carpet) {
  gl.bindVertexArray(geometry.vao);
  gl.uniformMatrix4fv(solidUniforms.model, false, model);
  gl.uniform3fv(solidUniforms.color, color);
  gl.uniform3fv(solidUniforms.emissive, emissive || [0, 0, 0]);
  gl.uniform1f(solidUniforms.carpet, carpet ? 1 : 0);
  gl.drawElements(gl.TRIANGLES, geometry.count, gl.UNSIGNED_SHORT, 0);
}

function makeTexture() {
  const texture = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return texture;
}

function uploadCanvas(texture, source) {
  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
}

function labelCanvas(title, accent, width = 256, height = 64) {
  const label = document.createElement('canvas');
  label.width = width;
  label.height = height;
  const ctx = label.getContext('2d');
  ctx.fillStyle = '#0a0512';
  ctx.fillRect(0, 0, width, height);
  ctx.strokeStyle = accent;
  ctx.lineWidth = 4;
  ctx.strokeRect(2, 2, width - 4, height - 4);
  ctx.fillStyle = accent;
  ctx.font = `bold ${Math.floor(height * 0.5)}px "Courier New", monospace`;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(title, width / 2, height / 2 + 2);
  return label;
}

const machines = [
  new MACHINE_TYPES.Rush(),
  new MACHINE_TYPES.Blocks(),
  new MACHINE_TYPES.Snake(),
  new MACHINE_TYPES.Nintendo(),
];

const cabinets = machines.map((machine, index) => {
  const canvasEl = MACHINE_TYPES.screenCanvas();
  const spacing = 5.4;
  return {
    machine,
    position: [(index - (machines.length - 1) / 2) * spacing, 0, -ROOM_Z + 1.6],
    yaw: 0,
    canvas: canvasEl,
    ctx: canvasEl.getContext('2d'),
    texture: makeTexture(),
    marquee: makeTexture(),
    marqueeSource: labelCanvas(machine.title, machine.accent),
    accent: hexToRgb(machine.accent),
    refresh: index,
  };
});
cabinets.forEach((cabinet) => uploadCanvas(cabinet.marquee, cabinet.marqueeSource));

// Dead cabinets along the side walls: scenery, no logic.
const scenery = [];
for (let i = 0; i < 8; i++) {
  const side = i < 4 ? -1 : 1;
  const slot = i % 4;
  scenery.push({
    position: [side * (ROOM_X - 1.4), 0, -5 + slot * 3.4],
    yaw: side * Math.PI * 0.5,
    hue: [[0.9, 0.2, 0.5], [0.2, 0.8, 0.9], [0.9, 0.7, 0.2], [0.5, 0.3, 1.0]][slot],
  });
}

const board = {
  canvas: labelCanvas('HIGH SCORES', '#ffe066', 512, 512),
  texture: makeTexture(),
};
uploadCanvas(board.texture, board.canvas);

function hexToRgb(hex) {
  const value = parseInt(hex.slice(1), 16);
  return [((value >> 16) & 255) / 255, ((value >> 8) & 255) / 255, (value & 255) / 255];
}

const state = {
  time: 0,
  player: { x: 0, z: 6, yaw: 0, pitch: 0 },
  keys: new Set(),
  seated: null,
  seatBlend: 0,
  freeCamera: null,
  prompt: null,
  saving: false,
};

const ui = {
  prompt: document.getElementById('prompt'),
  hud: document.getElementById('hud'),
  title: document.getElementById('machineTitle'),
  score: document.getElementById('machineScore'),
  hint: document.getElementById('machineHint'),
  intro: document.getElementById('introScreen'),
  save: document.getElementById('saveScreen'),
  saveCabinet: document.getElementById('saveCabinet'),
  saveScore: document.getElementById('saveScore'),
  saveNote: document.getElementById('saveNote'),
  nameInput: document.getElementById('nameInput'),
  cabinetScreen: document.getElementById('cabinetScreen'),
};
const cabinetScreenCtx = ui.cabinetScreen.getContext('2d');

function machineInput(attended) {
  if (!attended) return { attended: false };
  const keys = state.keys;
  return {
    attended: true,
    up: keys.has('ArrowUp') || keys.has('KeyW'),
    down: keys.has('ArrowDown') || keys.has('KeyS'),
    left: keys.has('ArrowLeft') || keys.has('KeyA'),
    right: keys.has('ArrowRight') || keys.has('KeyD'),
    fire: keys.has('Space') || keys.has('KeyZ'),
    secondary: keys.has('KeyX'),
    start: keys.has('Enter'),
    select: keys.has('ShiftLeft') || keys.has('ShiftRight'),
  };
}

function cabinetForward(cabinet) {
  return [Math.sin(cabinet.yaw), 0, Math.cos(cabinet.yaw)];
}

// In range and roughly in view: standing between two machines offers the one
// you are actually looking at.
function nearestCabinet() {
  const facingX = Math.sin(state.player.yaw);
  const facingZ = -Math.cos(state.player.yaw);
  let best = null;
  let bestScore = Infinity;
  for (const cabinet of cabinets) {
    const dx = cabinet.position[0] - state.player.x;
    const dz = cabinet.position[2] - state.player.z;
    const distance = Math.hypot(dx, dz);
    if (distance > REACH) continue;
    const alignment = (dx * facingX + dz * facingZ) / (distance || 1);
    if (alignment < 0.3) continue;
    const score = distance - alignment * 1.5;
    if (score < bestScore) {
      bestScore = score;
      best = cabinet;
    }
  }
  return best;
}

function sitDown(cabinet) {
  state.seated = cabinet;
  state.seatBlend = 0;
  state.freeCamera = { ...state.player };
  ui.hud.classList.add('show');
  ui.cabinetScreen.classList.add('show');
  ui.title.textContent = cabinet.machine.title;
  ui.hint.textContent =
    cabinet.machine.id === 'nes'
      ? 'arrows move / Z=B X=A enter=start shift=select / C swaps cartridge / Q leaves'
      : 'arrows or WASD move / space fires / enter restarts / Q leaves';
}

function standUp() {
  const cabinet = state.seated;
  state.seated = null;
  ui.hud.classList.remove('show');
  ui.cabinetScreen.classList.remove('show');
  if (!cabinet) return;
  const machine = cabinet.machine;
  if (machine.score !== null && machine.over && machine.score > 0) {
    offerToSave(cabinet);
  }
}

function offerToSave(cabinet) {
  state.saving = true;
  document.exitPointerLock();
  ui.saveCabinet.textContent = cabinet.machine.title;
  ui.saveScore.textContent = cabinet.machine.score;
  ui.saveNote.textContent = '';
  ui.save.dataset.cabinet = cabinet.machine.id;
  ui.save.dataset.score = cabinet.machine.score;
  ui.save.classList.add('show');
  cabinet.machine.reset();
}

function updatePlayer(dt) {
  const player = state.player;
  const forwardX = Math.sin(player.yaw), forwardZ = -Math.cos(player.yaw);
  let moveX = 0, moveZ = 0;
  if (state.keys.has('KeyW')) { moveX += forwardX; moveZ += forwardZ; }
  if (state.keys.has('KeyS')) { moveX -= forwardX; moveZ -= forwardZ; }
  if (state.keys.has('KeyD')) { moveX += Math.cos(player.yaw); moveZ += Math.sin(player.yaw); }
  if (state.keys.has('KeyA')) { moveX -= Math.cos(player.yaw); moveZ -= Math.sin(player.yaw); }

  const length = Math.hypot(moveX, moveZ);
  if (length > 0) {
    player.x += (moveX / length) * WALK_SPEED * dt;
    player.z += (moveZ / length) * WALK_SPEED * dt;
  }

  player.x = Math.max(-ROOM_X + PLAYER_RADIUS, Math.min(ROOM_X - PLAYER_RADIUS, player.x));
  player.z = Math.max(-ROOM_Z + PLAYER_RADIUS, Math.min(ROOM_Z - PLAYER_RADIUS, player.z));

  for (const obstacle of [...cabinets, ...scenery]) {
    const halfX = Math.abs(Math.cos(obstacle.yaw)) * 0.9 + Math.abs(Math.sin(obstacle.yaw)) * 0.7;
    const halfZ = Math.abs(Math.cos(obstacle.yaw)) * 0.7 + Math.abs(Math.sin(obstacle.yaw)) * 0.9;
    const dx = player.x - obstacle.position[0];
    const dz = player.z - obstacle.position[2];
    const overlapX = halfX + PLAYER_RADIUS - Math.abs(dx);
    const overlapZ = halfZ + PLAYER_RADIUS - Math.abs(dz);
    if (overlapX > 0 && overlapZ > 0) {
      if (overlapX < overlapZ) {
        player.x += Math.sign(dx || 1) * overlapX;
      } else {
        player.z += Math.sign(dz || 1) * overlapZ;
      }
    }
  }
}

function update(dt) {
  state.time += dt;

  if (!state.seated && !state.saving) {
    updatePlayer(dt);
    const near = nearestCabinet();
    state.prompt = near;
    ui.prompt.textContent = near ? `PRESS E — ${near.machine.title}` : '';
    ui.prompt.classList.toggle('show', Boolean(near));
  } else {
    ui.prompt.classList.remove('show');
  }

  if (state.seated) {
    state.seatBlend = Math.min(1, state.seatBlend + dt * 3);
    const machine = state.seated.machine;
    if (machine.score !== null) {
      ui.score.textContent = machine.score;
    } else {
      ui.score.textContent = machine.cartridge || '';
    }
  } else if (state.seatBlend > 0) {
    state.seatBlend = Math.max(0, state.seatBlend - dt * 3);
  }

  for (const cabinet of cabinets) {
    const attended = state.seated === cabinet;
    cabinet.machine.update(dt, machineInput(attended));
  }
}

// Cabinets face along (sin, cos) while the camera looks along (sin, -cos), so
// facing a cabinet means negating its yaw. The screen sits above eye level.
function seatCamera(cabinet) {
  const forward = cabinetForward(cabinet);
  return {
    x: cabinet.position[0] + forward[0] * 2.6,
    z: cabinet.position[2] + forward[2] * 2.6,
    yaw: -cabinet.yaw,
    pitch: 0.16,
  };
}

function camera() {
  const player = state.player;
  if (!state.seated && state.seatBlend === 0) return player;
  const target = state.seated ? seatCamera(state.seated) : player;
  const from = state.seated ? state.freeCamera || player : player;
  const t = state.seated ? state.seatBlend : 1;
  const lerp = (a, b) => a + (b - a) * t;
  let yawFrom = from.yaw;
  // Take the short way round, so sitting down never spins the room.
  while (target.yaw - yawFrom > Math.PI) yawFrom += Math.PI * 2;
  while (target.yaw - yawFrom < -Math.PI) yawFrom -= Math.PI * 2;
  return {
    x: lerp(from.x, target.x),
    z: lerp(from.z, target.z),
    yaw: lerp(yawFrom, target.yaw),
    pitch: lerp(from.pitch, target.pitch),
  };
}

function drawCabinetBody(position, yaw, hue, alive) {
  const body = [1.7, 3.1, 1.3];
  drawSolid(cube, m4box([position[0], body[1] / 2, position[2]], body, yaw), [0.17, 0.15, 0.22]);

  const forward = [Math.sin(yaw), 0, Math.cos(yaw)];
  const right = [Math.cos(yaw), 0, -Math.sin(yaw)];
  const at = (up, out, side) => [
    position[0] + forward[0] * out + right[0] * side,
    up,
    position[2] + forward[2] * out + right[2] * side,
  ];

  drawSolid(
    cube,
    m4box(at(1.02, 0.5, 0), [1.66, 0.24, 0.44], yaw, 0.45),
    [0.13, 0.12, 0.17],
    alive ? hue.map((c) => c * 0.10) : [0, 0, 0],
  );
  for (const side of [-0.86, 0.86]) {
    drawSolid(
      cube,
      m4box(at(1.9, 0.55, side), [0.06, 2.2, 0.06], yaw),
      [0, 0, 0],
      hue.map((c) => c * (alive ? 1.5 : 0.25)),
    );
  }
  drawSolid(cube, m4box(at(3.35, 0.1, 0), [1.75, 0.5, 1.1], yaw), [0.13, 0.11, 0.18]);
}

function render() {
  const width = canvas.clientWidth, height = canvas.clientHeight;
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.clearColor(0.035, 0.028, 0.062, 1);
  gl.enable(gl.DEPTH_TEST);
  gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);

  const view = camera();
  const eye = [view.x, EYE_HEIGHT, view.z];
  const forward = normalize([
    Math.sin(view.yaw) * Math.cos(view.pitch),
    Math.sin(view.pitch),
    -Math.cos(view.yaw) * Math.cos(view.pitch),
  ]);
  const right = normalize(cross(forward, [0, 1, 0]));
  const up = cross(right, forward);
  const viewProj = m4mul(
    m4perspective(Math.PI / 3, canvas.width / canvas.height, 0.08, 120),
    m4view(eye, right, up, forward),
  );

  gl.useProgram(solid);
  gl.uniformMatrix4fv(solidUniforms.viewProj, false, viewProj);
  gl.uniform3fv(solidUniforms.eye, eye);

  drawSolid(cube, m4box([0, -0.05, 0], [ROOM_X * 2, 0.1, ROOM_Z * 2], 0), [0.20, 0.10, 0.27], null, true);
  drawSolid(cube, m4box([0, ROOM_Y, 0], [ROOM_X * 2, 0.1, ROOM_Z * 2], 0), [0.11, 0.10, 0.15]);
  drawSolid(cube, m4box([0, ROOM_Y / 2, -ROOM_Z], [ROOM_X * 2, ROOM_Y, 0.2], 0), [0.19, 0.16, 0.26]);
  drawSolid(cube, m4box([0, ROOM_Y / 2, ROOM_Z], [ROOM_X * 2, ROOM_Y, 0.2], 0), [0.19, 0.16, 0.26]);
  drawSolid(cube, m4box([-ROOM_X, ROOM_Y / 2, 0], [0.2, ROOM_Y, ROOM_Z * 2], 0), [0.19, 0.16, 0.26]);
  drawSolid(cube, m4box([ROOM_X, ROOM_Y / 2, 0], [0.2, ROOM_Y, ROOM_Z * 2], 0), [0.19, 0.16, 0.26]);

  for (const x of [-8, 8]) {
    drawSolid(
      cube,
      m4box([x, ROOM_Y - 0.35, 0], [0.35, 0.12, ROOM_Z * 1.7], 0),
      [0, 0, 0],
      [1.1, 0.45, 1.4],
    );
  }

  for (const cabinet of scenery) {
    drawCabinetBody(cabinet.position, cabinet.yaw, cabinet.hue, false);
  }
  for (const cabinet of cabinets) {
    drawCabinetBody(cabinet.position, cabinet.yaw, cabinet.accent, true);
  }

  gl.useProgram(screen);
  gl.uniformMatrix4fv(screenUniforms.viewProj, false, viewProj);
  gl.uniform1i(screenUniforms.sampler, 0);
  gl.activeTexture(gl.TEXTURE0);

  for (const cabinet of cabinets) {
    const forwardAxis = cabinetForward(cabinet);
    const screenAt = [
      cabinet.position[0] + forwardAxis[0] * 0.63,
      2.1,
      cabinet.position[2] + forwardAxis[2] * 0.63,
    ];
    gl.bindTexture(gl.TEXTURE_2D, cabinet.texture);
    gl.uniform1f(screenUniforms.glow, state.seated === cabinet ? 1.25 : 1.0);
    gl.uniformMatrix4fv(
      screenUniforms.model,
      false,
      m4box(screenAt, [1.34, 1.2, 1], cabinet.yaw, -0.12),
    );
    gl.bindVertexArray(panel.vao);
    gl.drawElements(gl.TRIANGLES, panel.count, gl.UNSIGNED_SHORT, 0);

    const marqueeAt = [
      cabinet.position[0] + forwardAxis[0] * 0.66,
      3.35,
      cabinet.position[2] + forwardAxis[2] * 0.66,
    ];
    gl.bindTexture(gl.TEXTURE_2D, cabinet.marquee);
    gl.uniform1f(screenUniforms.glow, 1.4);
    gl.uniformMatrix4fv(
      screenUniforms.model,
      false,
      m4box(marqueeAt, [1.6, 0.45, 1], cabinet.yaw),
    );
    gl.drawElements(gl.TRIANGLES, panel.count, gl.UNSIGNED_SHORT, 0);
  }

  gl.bindTexture(gl.TEXTURE_2D, board.texture);
  gl.uniform1f(screenUniforms.glow, 1.15);
  gl.uniformMatrix4fv(
    screenUniforms.model,
    false,
    m4box([0, 3.0, ROOM_Z - 0.12], [4.2, 4.2, 1], Math.PI),
  );
  gl.bindVertexArray(panel.vao);
  gl.drawElements(gl.TRIANGLES, panel.count, gl.UNSIGNED_SHORT, 0);
}

function refreshScreens() {
  for (const cabinet of cabinets) {
    const attended = state.seated === cabinet;
    cabinet.refresh += 1;
    // Distant screens only need to look alive, not keep up.
    if (!attended && cabinet.refresh % 3 !== 0) continue;
    cabinet.machine.draw(cabinet.ctx);
    uploadCanvas(cabinet.texture, cabinet.canvas);
    if (attended) {
      cabinetScreenCtx.drawImage(cabinet.canvas, 0, 0);
    }
  }
}

let lastFrame = performance.now();
function frame(now) {
  const dt = Math.min((now - lastFrame) / 1000, 0.05);
  lastFrame = now;
  update(dt);
  refreshScreens();
  render();
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

async function loadScores() {
  try {
    const response = await fetch('/api/scores');
    drawScoreboard(await response.json());
  } catch (error) {
    drawScoreboard({});
  }
}

function drawScoreboard(scores) {
  const size = 512;
  const ctx = board.canvas.getContext('2d');
  ctx.fillStyle = '#0a0512';
  ctx.fillRect(0, 0, size, size);
  ctx.strokeStyle = '#ffe066';
  ctx.lineWidth = 5;
  ctx.strokeRect(3, 3, size - 6, size - 6);
  ctx.textAlign = 'center';
  ctx.fillStyle = '#ffe066';
  ctx.font = 'bold 38px "Courier New", monospace';
  ctx.fillText('HIGH SCORES', size / 2, 52);

  let y = 100;
  for (const machine of machines) {
    if (machine.score === null) continue;
    ctx.fillStyle = machine.accent;
    ctx.font = 'bold 26px "Courier New", monospace';
    ctx.textAlign = 'left';
    ctx.fillText(machine.title, 34, y);
    const entries = (scores[machine.id] || []).slice(0, 4);
    ctx.font = '22px "Courier New", monospace';
    ctx.fillStyle = '#d8d2e8';
    if (!entries.length) {
      ctx.fillText('- no runs yet -', 54, y + 28);
      y += 66;
      continue;
    }
    entries.forEach((entry, index) => {
      ctx.textAlign = 'left';
      ctx.fillText(`${index + 1}. ${entry.name}`, 54, y + 28 + index * 26);
      ctx.textAlign = 'right';
      ctx.fillText(`${entry.score}`, size - 40, y + 28 + index * 26);
    });
    y += 40 + entries.length * 26;
  }
  uploadCanvas(board.texture, board.canvas);
}

function enter() {
  ui.intro.classList.remove('show');
  canvas.requestPointerLock();
}

document.getElementById('enterBtn').addEventListener('click', enter);

document.getElementById('saveForm').addEventListener('submit', async (event) => {
  event.preventDefault();
  ui.saveNote.textContent = 'saving...';
  try {
    const response = await fetch('/api/scores', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        cabinet: ui.save.dataset.cabinet,
        name: ui.nameInput.value || 'anon',
        score: Number(ui.save.dataset.score),
      }),
    });
    const payload = await response.json();
    if (!response.ok) {
      ui.saveNote.textContent = payload.error || 'rejected';
      return;
    }
    ui.saveNote.textContent = `ranked #${payload.rank}`;
    await loadScores();
    window.setTimeout(closeSave, 700);
  } catch (error) {
    ui.saveNote.textContent = 'server unreachable';
  }
});

function closeSave() {
  state.saving = false;
  ui.save.classList.remove('show');
  canvas.requestPointerLock();
}

document.getElementById('skipBtn').addEventListener('click', closeSave);

canvas.addEventListener('mousedown', () => {
  if (document.pointerLockElement !== canvas && !state.saving) canvas.requestPointerLock();
});

document.addEventListener('mousemove', (event) => {
  if (document.pointerLockElement !== canvas || state.seated) return;
  state.player.yaw += event.movementX * 0.0022;
  state.player.pitch = Math.max(
    -1.2,
    Math.min(1.2, state.player.pitch - event.movementY * 0.0022),
  );
});

document.addEventListener('keydown', (event) => {
  if (state.saving) return;
  state.keys.add(event.code);
  if (['Space', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.code)) {
    event.preventDefault();
  }
  if (event.code === 'KeyE' && !state.seated && state.prompt) {
    sitDown(state.prompt);
  }
  if ((event.code === 'KeyQ' || event.code === 'Escape') && state.seated) {
    standUp();
  }
  if (event.code === 'KeyC' && state.seated && state.seated.machine.nextCartridge) {
    state.seated.machine.nextCartridge();
  }
});
document.addEventListener('keyup', (event) => state.keys.delete(event.code));
document.addEventListener('pointerlockchange', () => {
  if (document.pointerLockElement !== canvas) state.keys.clear();
});

loadScores();
