"""Gymnasium environment for Chudtendo — structured feature observation.

Uses tile map + sprite positions + game state as a flat feature vector
instead of raw pixels. Much faster for an MLP to learn from.

Usage:
    cargo run --release --bin agent_server -- rom/sml.gb
    cd rl && python train.py
"""

import json
import socket
import time

import gymnasium as gym
import numpy as np
from gymnasium import spaces

# Tile grid: 20 columns x 18 rows visible on screen.
MAX_SPRITES = 10  # Max sprites to include in observation.


class ChudtendoEnv(gym.Env):
    """Game Boy environment with structured observations.

    Observation (flat float32 vector):
      - 10 sprites x 3 (x, y, tile), padded (30 values, normalized)
      - Game state: scx, scy, lives, coins, powerup, score (6 values)
    Total: 36 features.

    Action space: 14 discrete (movement x jump heights + down).
    """

    metadata = {"render_modes": ["rgb_array"]}

    ACTIONS = [
        ([], 4),                     #  0: noop
        (["right"], 4),              #  1: walk right
        (["right", "a"], 4),         #  2: right + short hop
        (["right", "a"], 10),        #  3: right + medium jump
        (["right", "a"], 18),        #  4: right + full jump
        (["a"], 4),                  #  5: short hop
        (["a"], 18),                 #  6: full jump
        (["left"], 4),               #  7: walk left
        (["left", "a"], 10),         #  8: left + medium jump
        (["right", "b"], 4),         #  9: sprint right
        (["right", "b", "a"], 10),   # 10: sprint + medium jump
        (["right", "b", "a"], 18),   # 11: sprint + full jump
        (["down"], 8),               # 12: duck / enter pipe
        (["down", "right"], 4),      # 13: crouch-walk right
    ]

    OBS_SIZE = MAX_SPRITES * 3 + 6  # 36

    def __init__(self, host="127.0.0.1", port=31337, render_mode=None):
        super().__init__()
        self.host = host
        self.port = port
        self.render_mode = render_mode

        self.action_space = spaces.Discrete(len(self.ACTIONS))
        self.observation_space = spaces.Box(
            low=0.0, high=1.0, shape=(self.OBS_SIZE,), dtype=np.float32
        )

        self._sock = None
        self._prev_scroll = 0
        self._prev_score = 0
        self._prev_coins = 0
        self._prev_lives = 0
        self._steps = 0
        self._max_steps = 4000
        self._total_scroll = 0
        self._no_progress_count = 0

    # --- Socket ---

    def _connect(self):
        if self._sock:
            try: self._sock.close()
            except: pass
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._sock.connect((self.host, self.port))
        self._sock.settimeout(10.0)

    def _send(self, cmd: str) -> dict:
        try:
            self._sock.sendall((cmd + "\n").encode())
            data = b""
            while b"\n" not in data:
                chunk = self._sock.recv(65536)
                if not chunk: break
                data += chunk
            return json.loads(data.strip())
        except (ConnectionError, OSError, json.JSONDecodeError):
            self._connect()
            return {"error": "reconnected"}

    # --- Observation ---

    def _build_obs(self, feat: dict) -> np.ndarray:
        obs = np.zeros(self.OBS_SIZE, dtype=np.float32)
        idx = 0

        # Sprites: up to MAX_SPRITES, each (x/160, y/144, tile/255).
        sprites = feat.get("sprites", [])
        for i in range(MAX_SPRITES):
            if i < len(sprites):
                s = sprites[i]
                obs[idx] = (s[0] + 8) / 176.0    # x normalized
                obs[idx+1] = (s[1] + 16) / 176.0  # y normalized
                obs[idx+2] = s[2] / 255.0          # tile id
            # else: zeros (padding)
            idx += 3

        # Game state.
        obs[idx] = feat.get("scx", 0) / 255.0
        obs[idx+1] = feat.get("scy", 0) / 255.0
        obs[idx+2] = feat.get("lives", 0) / 3.0
        obs[idx+3] = feat.get("coins", 0) / 99.0
        obs[idx+4] = feat.get("powerup", 0) / 2.0
        obs[idx+5] = min(feat.get("score", 0) / 100000.0, 1.0)

        return obs

    # --- Input ---

    def _press_buttons(self, buttons: list[str], hold_frames: int):
        if not buttons:
            self._send(f"frames {hold_frames}")
            return
        # Press buttons, advance exact game frames, release.
        for btn in buttons:
            self._send(f"down {btn}")
        self._send(f"frames {hold_frames}")
        for btn in buttons:
            self._send(f"up {btn}")

    # --- Reward ---

    def _compute_reward(self, feat: dict) -> tuple[float, bool]:
        reward = 0.0
        terminated = False

        # Scroll progress.
        scroll = feat.get("scx", 0)
        delta = scroll - self._prev_scroll
        if delta < -128: delta += 256
        elif delta > 128: delta -= 256
        if delta > 0:
            reward += delta * 0.1
            self._total_scroll += delta
            self._no_progress_count = 0
        else:
            self._no_progress_count += 1
        self._prev_scroll = scroll

        # Score (kills, powerups, blocks, coins, level end).
        score = feat.get("score", 0)
        score_delta = score - self._prev_score
        if 0 < score_delta < 50000:
            reward += score_delta * 0.005
            self._no_progress_count = 0  # Score counts as progress
        self._prev_score = score

        # Coins.
        coins = feat.get("coins", 0)
        coin_delta = coins - self._prev_coins
        if 0 < coin_delta < 50:
            reward += coin_delta * 0.5
            self._no_progress_count = 0  # Coins count as progress
        self._prev_coins = coins

        # Death.
        lives = feat.get("lives", 0)
        if lives < self._prev_lives and self._prev_lives > 0:
            reward -= 15.0
        self._prev_lives = lives

        # Game over.
        if lives == 0:
            sprites = feat.get("sprites", [])
            if len(sprites) == 0 or self._no_progress_count > 30:
                terminated = True
                reward -= 25.0

        # Stuck (no scroll, no score, no coins for a long time).
        if self._no_progress_count > 200:
            reward -= 5.0
            terminated = True

        return reward, terminated

    # --- Gym interface ---

    def reset(self, *, seed=None, options=None):
        super().reset(seed=seed)
        if self._sock is None:
            self._connect()

        resp = self._send("load 8")
        if "error" in resp:
            # No save state yet — start from title screen.
            self._send("press start 300")
            time.sleep(1.0)
            self._send("press start 300")
            time.sleep(3.0)
            # Wait for gameplay to begin.
            self._send("frames 60")
            self._send("save 8")

        # Give components time to process the loaded state.
        time.sleep(0.3)
        # Advance a few frames so the PPU renders with the restored state.
        self._send("frames 10")

        feat = self._send("features")
        self._prev_scroll = feat.get("scx", 0)
        self._prev_score = feat.get("score", 0)
        self._prev_coins = feat.get("coins", 0)
        self._prev_lives = feat.get("lives", 3)
        self._steps = 0
        self._total_scroll = 0
        self._no_progress_count = 0

        obs = self._build_obs(feat)
        return obs, {"lives": self._prev_lives, "score": self._prev_score}

    def step(self, action: int):
        buttons, hold_frames = self.ACTIONS[action]
        self._press_buttons(buttons, hold_frames)
        self._steps += 1

        feat = self._send("features")
        reward, terminated = self._compute_reward(feat)
        truncated = self._steps >= self._max_steps
        obs = self._build_obs(feat)

        info = {
            "lives": self._prev_lives,
            "score": self._prev_score,
            "coins": self._prev_coins,
            "scroll": self._total_scroll,
            "steps": self._steps,
        }
        return obs, reward, terminated, truncated, info

    def render(self):
        return None

    def close(self):
        if self._sock:
            try: self._sock.close()
            except: pass
            self._sock = None
