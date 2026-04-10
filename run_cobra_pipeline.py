#!/usr/bin/env python3

import argparse
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
BIN_DIR = ROOT / "target" / "release"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run Cobra binaries sequentially and stop immediately if one step fails."
        )
    )
    parser.add_argument(
        "config",
        nargs="?",
        default="config.yml",
        help="Path to the Cobra YAML config file. Defaults to ./config.yml.",
    )
    parser.add_argument(
        "--bin-dir",
        default=str(BIN_DIR),
        help="Directory containing the compiled binaries. Defaults to ./target/release.",
    )
    return parser.parse_args()


def run_step(binary_path: Path, config_path: Path) -> None:
    command = [str(binary_path), str(config_path)]
    print(f"\n=== Running {binary_path.name} ===", flush=True)
    print(" ".join(command), flush=True)
    subprocess.run(command, check=True, cwd=ROOT, env=os.environ.copy())
    print(f"=== {binary_path.name} finished successfully ===", flush=True)


def main() -> int:
    args = parse_args()
    config_path = Path(args.config).expanduser()
    if not config_path.is_absolute():
        config_path = (ROOT / config_path).resolve()

    if not config_path.exists():
        print(f"Config file not found: {config_path}", file=sys.stderr)
        return 1

    bin_dir = Path(args.bin_dir).expanduser()
    if not bin_dir.is_absolute():
        bin_dir = (ROOT / bin_dir).resolve()

    binaries = [
        "imap-rusty-reader",
        "parse-ia",
        "ai-enrich",
        "dropbox-filer",
    ]

    for binary_name in binaries:
        binary_path = bin_dir / binary_name
        if not binary_path.exists():
            print(f"Binary not found: {binary_path}", file=sys.stderr)
            return 1
        if not os.access(binary_path, os.X_OK):
            print(f"Binary is not executable: {binary_path}", file=sys.stderr)
            return 1

    try:
        for binary_name in binaries:
            run_step(bin_dir / binary_name, config_path)
    except subprocess.CalledProcessError as exc:
        print(
            f"\nPipeline stopped because {Path(exc.cmd[0]).name} exited with code {exc.returncode}.",
            file=sys.stderr,
        )
        return exc.returncode

    print("\nCobra pipeline completed successfully.", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
