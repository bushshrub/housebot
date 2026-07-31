'use strict';

// Every cabinet screen is a NES-sized canvas. Machines draw into one of these
// whether or not anybody is playing, so the room is full of moving screens.
const SCREEN_W = 256;
const SCREEN_H = 240;

function screenCanvas() {
  const canvas = document.createElement('canvas');
  canvas.width = SCREEN_W;
  canvas.height = SCREEN_H;
  return canvas;
}

function text(ctx, value, x, y, size, color, align = 'center') {
  ctx.fillStyle = color;
  ctx.font = `${size}px "Courier New", monospace`;
  ctx.textAlign = align;
  ctx.fillText(value, x, y);
}

function fade(ctx, color, alpha) {
  ctx.globalAlpha = alpha;
  ctx.fillStyle = color;
  ctx.fillRect(0, 0, SCREEN_W, SCREEN_H);
  ctx.globalAlpha = 1;
}

class Rush {
  constructor() {
    this.id = 'rush';
    this.title = 'RUSH';
    this.accent = '#ff2e88';
    this.blurb = 'Hold the floor. They rush back in.';
    this.reset();
  }

  reset() {
    this.player = { x: SCREEN_W / 2, y: SCREEN_H / 2, aim: { x: 0, y: -1 } };
    this.enemies = [];
    this.shots = [];
    this.score = 0;
    this.wave = 1;
    this.lives = 3;
    this.spawn = 0;
    this.pending = 5;
    this.over = false;
    this.cooldown = 0;
    this.flash = 0;
  }

  update(dt, input) {
    if (this.over) {
      this.flash += dt;
      if (input.start && this.flash > 0.6) this.reset();
      return;
    }

    let dx = (input.right ? 1 : 0) - (input.left ? 1 : 0);
    let dy = (input.down ? 1 : 0) - (input.up ? 1 : 0);
    const length = Math.hypot(dx, dy);
    if (length > 0) {
      dx /= length;
      dy /= length;
      this.player.aim = { x: dx, y: dy };
      this.player.x = Math.max(8, Math.min(SCREEN_W - 8, this.player.x + dx * 78 * dt));
      this.player.y = Math.max(8, Math.min(SCREEN_H - 8, this.player.y + dy * 78 * dt));
    }

    this.cooldown -= dt;
    if (input.fire && this.cooldown <= 0) {
      this.cooldown = 0.22;
      this.shots.push({
        x: this.player.x,
        y: this.player.y,
        dx: this.player.aim.x * 190,
        dy: this.player.aim.y * 190,
      });
    }

    this.spawn -= dt;
    if (this.pending > 0 && this.spawn <= 0) {
      this.spawn = 0.7;
      this.pending -= 1;
      const edge = Math.floor(Math.random() * 4);
      this.enemies.push({
        x: edge === 0 ? 0 : edge === 1 ? SCREEN_W : Math.random() * SCREEN_W,
        y: edge === 2 ? 0 : edge === 3 ? SCREEN_H : Math.random() * SCREEN_H,
        speed: 22 + this.wave * 4,
      });
    }

    for (const shot of this.shots) {
      shot.x += shot.dx * dt;
      shot.y += shot.dy * dt;
    }
    this.shots = this.shots.filter(
      (shot) => shot.x > -4 && shot.x < SCREEN_W + 4 && shot.y > -4 && shot.y < SCREEN_H + 4,
    );

    for (const enemy of this.enemies) {
      const toX = this.player.x - enemy.x;
      const toY = this.player.y - enemy.y;
      const distance = Math.hypot(toX, toY) || 1;
      enemy.x += (toX / distance) * enemy.speed * dt;
      enemy.y += (toY / distance) * enemy.speed * dt;

      if (distance < 9) {
        enemy.dead = true;
        this.lives -= 1;
        this.flash = 0.25;
        if (this.lives <= 0) this.over = true;
      }
      for (const shot of this.shots) {
        if (Math.hypot(shot.x - enemy.x, shot.y - enemy.y) < 7) {
          enemy.dead = true;
          shot.spent = true;
          this.score += 10 * this.wave;
        }
      }
    }
    this.enemies = this.enemies.filter((enemy) => !enemy.dead);
    this.shots = this.shots.filter((shot) => !shot.spent);
    this.flash = Math.max(0, this.flash - dt);

    if (this.pending === 0 && this.enemies.length === 0) {
      this.score += 50 * this.wave;
      this.wave += 1;
      this.pending = 3 + this.wave * 2;
    }
  }

  draw(ctx) {
    ctx.fillStyle = '#0d0820';
    ctx.fillRect(0, 0, SCREEN_W, SCREEN_H);

    ctx.strokeStyle = 'rgba(56,242,208,0.18)';
    ctx.lineWidth = 1;
    for (let x = 0; x <= SCREEN_W; x += 16) {
      ctx.beginPath();
      ctx.moveTo(x + 0.5, 0);
      ctx.lineTo(x + 0.5, SCREEN_H);
      ctx.stroke();
    }
    for (let y = 0; y <= SCREEN_H; y += 16) {
      ctx.beginPath();
      ctx.moveTo(0, y + 0.5);
      ctx.lineTo(SCREEN_W, y + 0.5);
      ctx.stroke();
    }

    ctx.fillStyle = '#38f2d0';
    ctx.fillRect(this.player.x - 5, this.player.y - 5, 10, 10);
    ctx.fillStyle = '#0b0718';
    ctx.fillRect(
      this.player.x - 2 + this.player.aim.x * 4,
      this.player.y - 2 + this.player.aim.y * 4,
      4,
      4,
    );

    ctx.fillStyle = '#ff2e88';
    for (const enemy of this.enemies) ctx.fillRect(enemy.x - 4, enemy.y - 4, 8, 8);
    ctx.fillStyle = '#ffe066';
    for (const shot of this.shots) ctx.fillRect(shot.x - 1.5, shot.y - 1.5, 3, 3);

    if (this.flash > 0) fade(ctx, '#ff2e88', Math.min(this.flash, 0.5));

    text(ctx, `${this.score}`, 8, 16, 14, '#38f2d0', 'left');
    text(ctx, `W${this.wave}  ${'|'.repeat(Math.max(0, this.lives))}`, SCREEN_W - 8, 16, 14, '#ff2e88', 'right');
    if (this.over) {
      fade(ctx, '#06030f', 0.7);
      text(ctx, 'GAME OVER', SCREEN_W / 2, SCREEN_H / 2, 24, '#ff2e88');
      text(ctx, `${this.score}`, SCREEN_W / 2, SCREEN_H / 2 + 26, 16, '#fff');
    }
  }
}

class Blocks {
  constructor() {
    this.id = 'blocks';
    this.title = 'BLOCKS';
    this.accent = '#38f2d0';
    this.blurb = 'Clear the wall. Do not drop it.';
    this.reset();
  }

  reset() {
    this.paddle = SCREEN_W / 2;
    this.ball = { x: SCREEN_W / 2, y: SCREEN_H - 40, dx: 92, dy: -92 };
    this.score = 0;
    this.lives = 3;
    this.over = false;
    this.bricks = [];
    for (let row = 0; row < 5; row++) {
      for (let column = 0; column < 8; column++) {
        this.bricks.push({ x: 8 + column * 30, y: 30 + row * 14, row, alive: true });
      }
    }
  }

  update(dt, input) {
    if (this.over) {
      if (input.start) this.reset();
      return;
    }

    const speed = 150;
    if (input.left) this.paddle -= speed * dt;
    if (input.right) this.paddle += speed * dt;
    // With no hands on the controls the paddle tracks the ball, so the cabinet
    // demos itself.
    if (!input.left && !input.right && !input.attended) {
      this.paddle += Math.sign(this.ball.x - this.paddle) * speed * 0.55 * dt;
    }
    this.paddle = Math.max(20, Math.min(SCREEN_W - 20, this.paddle));

    const ball = this.ball;
    ball.x += ball.dx * dt;
    ball.y += ball.dy * dt;
    if (ball.x < 4 || ball.x > SCREEN_W - 4) ball.dx *= -1;
    if (ball.y < 20) ball.dy = Math.abs(ball.dy);

    if (ball.y > SCREEN_H - 18 && ball.y < SCREEN_H - 10 && Math.abs(ball.x - this.paddle) < 22) {
      ball.dy = -Math.abs(ball.dy);
      ball.dx += (ball.x - this.paddle) * 1.6;
      ball.dx = Math.max(-190, Math.min(190, ball.dx));
    }

    if (ball.y > SCREEN_H) {
      this.lives -= 1;
      ball.x = SCREEN_W / 2;
      ball.y = SCREEN_H - 40;
      ball.dx = 92 * (Math.random() < 0.5 ? -1 : 1);
      ball.dy = -92;
      if (this.lives <= 0) this.over = true;
    }

    for (const brick of this.bricks) {
      if (!brick.alive) continue;
      if (ball.x > brick.x && ball.x < brick.x + 28 && ball.y > brick.y && ball.y < brick.y + 12) {
        brick.alive = false;
        ball.dy *= -1;
        this.score += (5 - brick.row) * 10;
      }
    }
    if (this.bricks.every((brick) => !brick.alive)) {
      for (const brick of this.bricks) brick.alive = true;
      ball.dx *= 1.1;
      ball.dy *= 1.1;
    }
  }

  draw(ctx) {
    ctx.fillStyle = '#04070f';
    ctx.fillRect(0, 0, SCREEN_W, SCREEN_H);

    const hues = ['#ff2e88', '#ff8a3d', '#ffe066', '#38f2d0', '#7b5cff'];
    for (const brick of this.bricks) {
      if (!brick.alive) continue;
      ctx.fillStyle = hues[brick.row];
      ctx.fillRect(brick.x, brick.y, 28, 12);
    }

    ctx.fillStyle = '#38f2d0';
    ctx.fillRect(this.paddle - 20, SCREEN_H - 16, 40, 5);
    ctx.fillStyle = '#fff';
    ctx.fillRect(this.ball.x - 3, this.ball.y - 3, 6, 6);

    text(ctx, `${this.score}`, 8, 16, 14, '#38f2d0', 'left');
    text(ctx, '*'.repeat(Math.max(0, this.lives)), SCREEN_W - 8, 16, 14, '#ff2e88', 'right');
    if (this.over) {
      fade(ctx, '#04070f', 0.7);
      text(ctx, 'GAME OVER', SCREEN_W / 2, SCREEN_H / 2, 24, '#38f2d0');
      text(ctx, `${this.score}`, SCREEN_W / 2, SCREEN_H / 2 + 26, 16, '#fff');
    }
  }
}

class Snake {
  constructor() {
    this.id = 'snake';
    this.title = 'SNAKE';
    this.accent = '#7bff5c';
    this.blurb = 'Eat. Grow. Regret.';
    this.cell = 16;
    this.columns = SCREEN_W / this.cell;
    this.rows = (SCREEN_H - 24) / this.cell;
    this.reset();
  }

  reset() {
    this.body = [{ x: 6, y: 7 }, { x: 5, y: 7 }, { x: 4, y: 7 }];
    this.direction = { x: 1, y: 0 };
    this.queued = { x: 1, y: 0 };
    this.food = { x: 12, y: 7 };
    this.timer = 0;
    this.score = 0;
    this.over = false;
  }

  update(dt, input) {
    if (this.over) {
      if (input.start) this.reset();
      return;
    }

    if (input.up && this.direction.y === 0) this.queued = { x: 0, y: -1 };
    if (input.down && this.direction.y === 0) this.queued = { x: 0, y: 1 };
    if (input.left && this.direction.x === 0) this.queued = { x: -1, y: 0 };
    if (input.right && this.direction.x === 0) this.queued = { x: 1, y: 0 };

    this.timer += dt;
    const step = 0.13;
    if (this.timer < step) return;
    this.timer -= step;

    this.direction = this.queued;
    const head = {
      x: this.body[0].x + this.direction.x,
      y: this.body[0].y + this.direction.y,
    };

    if (
      head.x < 0 || head.y < 0 || head.x >= this.columns || head.y >= this.rows ||
      this.body.some((part) => part.x === head.x && part.y === head.y)
    ) {
      this.over = true;
      return;
    }

    this.body.unshift(head);
    if (head.x === this.food.x && head.y === this.food.y) {
      this.score += 10;
      do {
        this.food = {
          x: Math.floor(Math.random() * this.columns),
          y: Math.floor(Math.random() * this.rows),
        };
      } while (this.body.some((part) => part.x === this.food.x && part.y === this.food.y));
    } else {
      this.body.pop();
    }
  }

  draw(ctx) {
    ctx.fillStyle = '#0a1a10';
    ctx.fillRect(0, 0, SCREEN_W, SCREEN_H);

    const top = 24;
    ctx.strokeStyle = 'rgba(123,255,92,0.15)';
    ctx.strokeRect(0.5, top + 0.5, SCREEN_W - 1, SCREEN_H - top - 1);

    ctx.fillStyle = '#ff2e88';
    ctx.fillRect(this.food.x * this.cell + 3, top + this.food.y * this.cell + 3, this.cell - 6, this.cell - 6);

    this.body.forEach((part, index) => {
      ctx.fillStyle = index === 0 ? '#c8ff8a' : '#7bff5c';
      ctx.fillRect(part.x * this.cell + 1, top + part.y * this.cell + 1, this.cell - 2, this.cell - 2);
    });

    text(ctx, `${this.score}`, 8, 16, 14, '#7bff5c', 'left');
    if (this.over) {
      fade(ctx, '#050d08', 0.7);
      text(ctx, 'GAME OVER', SCREEN_W / 2, SCREEN_H / 2, 24, '#7bff5c');
      text(ctx, `${this.score}`, SCREEN_W / 2, SCREEN_H / 2 + 26, 16, '#fff');
    }
  }
}

// The one cabinet that is not a browser game: frames come from the NES core in
// the Rust backend, one HTTP round trip each.
class Nintendo {
  constructor() {
    this.id = 'nes';
    this.title = 'NES';
    this.accent = '#e8402a';
    this.blurb = 'Original hardware, re-created in Rust.';
    this.score = null;
    this.over = false;
    this.cartridges = [];
    this.cartridge = null;
    this.status = 'booting...';
    this.palette = null;
    this.image = null;
    this.inFlight = false;
    this.buttons = 0;
    this.ready = false;
    this.idleClock = 0;
    this.boot();
  }

  reset() {}

  async boot() {
    try {
      const [paletteResponse, listResponse] = await Promise.all([
        fetch('/api/nes/palette'),
        fetch('/api/nes/cartridges'),
      ]);
      this.palette = await paletteResponse.json();
      this.cartridges = (await listResponse.json()).cartridges;
      await this.insert(this.cartridges[0]);
    } catch (error) {
      this.status = 'backend offline';
    }
  }

  async insert(cartridge) {
    this.ready = false;
    this.status = `loading ${cartridge}`;
    const response = await fetch('/api/nes/insert', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ cartridge }),
    });
    if (!response.ok) {
      this.status = (await response.json()).error || 'bad cartridge';
      return;
    }
    this.cartridge = cartridge;
    this.ready = true;
  }

  nextCartridge() {
    if (this.cartridges.length < 2) return;
    const at = this.cartridges.indexOf(this.cartridge);
    this.insert(this.cartridges[(at + 1) % this.cartridges.length]);
  }

  update(dt, input) {
    if (!this.ready || this.inFlight) return;
    // Unattended, the cabinet only needs to look alive — don't hammer the
    // backend for 60 frames a second nobody is watching closely.
    this.idleClock += dt;
    if (!input.attended && this.idleClock < 0.12) return;
    this.idleClock = 0;

    // Bit order matches the shift register in nes/bus.rs.
    this.buttons =
      (input.fire ? 0x01 : 0) |
      (input.secondary ? 0x02 : 0) |
      (input.select ? 0x04 : 0) |
      (input.start ? 0x08 : 0) |
      (input.up ? 0x10 : 0) |
      (input.down ? 0x20 : 0) |
      (input.left ? 0x40 : 0) |
      (input.right ? 0x80 : 0);

    this.inFlight = true;
    fetch('/api/nes/frame', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ buttons: this.buttons, frames: input.attended ? 1 : 2 }),
    })
      .then((response) => (response.ok ? response.arrayBuffer() : null))
      .then((buffer) => {
        if (buffer) this.paint(new Uint8Array(buffer));
        this.inFlight = false;
      })
      .catch(() => {
        this.ready = false;
        this.status = 'backend offline';
        this.inFlight = false;
      });
  }

  paint(indices) {
    if (!this.palette) return;
    if (!this.image) this.image = new ImageData(SCREEN_W, SCREEN_H);
    const pixels = this.image.data;
    for (let i = 0; i < indices.length; i++) {
      const color = this.palette[indices[i] & 0x3F];
      pixels[i * 4] = color[0];
      pixels[i * 4 + 1] = color[1];
      pixels[i * 4 + 2] = color[2];
      pixels[i * 4 + 3] = 255;
    }
  }

  draw(ctx) {
    if (this.image) {
      ctx.putImageData(this.image, 0, 0);
      return;
    }
    ctx.fillStyle = '#101014';
    ctx.fillRect(0, 0, SCREEN_W, SCREEN_H);
    text(ctx, 'NINTENDO', SCREEN_W / 2, SCREEN_H / 2 - 10, 22, '#e8402a');
    text(ctx, this.status, SCREEN_W / 2, SCREEN_H / 2 + 16, 12, '#8a8a94');
  }
}

window.ARCADE_MACHINES = { Rush, Blocks, Snake, Nintendo, SCREEN_W, SCREEN_H, screenCanvas };
