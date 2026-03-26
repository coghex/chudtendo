"""Train an RL agent to play Super Mario Land using structured features.

Usage:
    1. Start the emulator:
       cargo run --release --bin agent_server -- --turbo=2.0 rom/sml.gb

    2. Run training:
       cd rl && python train.py

    TensorBoard: tensorboard --logdir logs
"""

from pathlib import Path

from stable_baselines3 import PPO
from stable_baselines3.common.callbacks import CheckpointCallback, BaseCallback
from stable_baselines3.common.vec_env import DummyVecEnv

from chudtendo_env import ChudtendoEnv


class EpisodeLogCallback(BaseCallback):
    def __init__(self, verbose=0):
        super().__init__(verbose)
        self.episode_count = 0
        self.ep_reward = 0.0

    def _on_step(self):
        rewards = self.locals.get("rewards", [])
        if len(rewards) > 0:
            self.ep_reward += rewards[0]

        dones = self.locals.get("dones", [])
        if len(dones) > 0 and dones[0]:
            self.episode_count += 1
            infos = self.locals.get("infos", [{}])
            info = infos[0] if infos else {}
            print(
                f"  ep {self.episode_count:4d} | "
                f"reward {self.ep_reward:7.1f} | "
                f"steps {info.get('steps', '?'):5} | "
                f"scroll {info.get('scroll', 0):5d} | "
                f"score {info.get('score', 0):6d} | "
                f"coins {info.get('coins', 0):2d} | "
                f"lives {info.get('lives', '?')}"
            )
            self.ep_reward = 0.0
        return True


def make_env():
    def _init():
        return ChudtendoEnv()
    return _init


def main():
    checkpoint_dir = Path("checkpoints")
    checkpoint_dir.mkdir(exist_ok=True)
    log_dir = Path("logs")
    log_dir.mkdir(exist_ok=True)

    env = DummyVecEnv([make_env()])

    model = PPO(
        "MlpPolicy",
        env,
        verbose=1,
        learning_rate=3e-4,
        n_steps=256,
        batch_size=64,
        n_epochs=4,
        gamma=0.99,
        gae_lambda=0.95,
        clip_range=0.2,
        ent_coef=0.02,
        vf_coef=0.5,
        max_grad_norm=0.5,
        policy_kwargs=dict(net_arch=[256, 256]),
        tensorboard_log=str(log_dir),
    )

    callbacks = [
        CheckpointCallback(save_freq=10_000, save_path=str(checkpoint_dir), name_prefix="sml"),
        EpisodeLogCallback(),
    ]

    print("Starting training (MLP + structured features)")
    print(f"  Observation: {ChudtendoEnv.OBS_SIZE} features (tiles + sprites + state)")
    print(f"  Actions: {len(ChudtendoEnv.ACTIONS)}")
    print(f"  Policy: MLP [256, 256]")
    print(f"  Checkpoints: {checkpoint_dir}/")
    print(f"  TensorBoard: tensorboard --logdir {log_dir}")
    print()

    try:
        model.learn(
            total_timesteps=1_000_000,
            callback=callbacks,
            progress_bar=True,
        )
        model.save("sml_final")
        print("Training complete. Model saved to sml_final.zip")
    except KeyboardInterrupt:
        model.save("sml_interrupted")
        print("\nSaved to sml_interrupted.zip")


if __name__ == "__main__":
    main()
