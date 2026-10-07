#!/usr/bin/env python3
"""Materialise a Rust toolchain from the `arena-rust-toolchain` PyPI bundle.

This sandbox cannot reach `static.rust-lang.org` (rustup) but it *can* reach
PyPI, where the toolchain is published as a package split into <=100 MB parts.
This script concatenates the parts, decompresses the zstd stream and extracts the
tarball to `/tmp/rust`, then makes the binaries executable (tar drops the exec
bit here).

    python3 tools/extract_toolchain.py [dest]      # default /tmp/rust

Afterwards:

    export PATH=/tmp/rust/prefix/bin:$PATH

Note: `/tmp` is not persisted between sessions, so re-run this whenever a new
shell needs cargo.  Nothing under `/home/user` is touched.
"""
from __future__ import annotations

import glob
import os
import shutil
import subprocess
import sys
import tarfile

PARTS = [
    "arena-rust-toolchain",
    "arena-rust-toolchain-data1",
    "arena-rust-toolchain-data2",
    "arena-rust-toolchain-data3",
]
STAGE = "/tmp/rtc"
LIBS = "/tmp/pylibs"


def sh(*cmd: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=check, capture_output=True, text=True)


def pip(*args: str) -> None:
    """`pip install --target=…` (PEP 668 blocks the system environment)."""
    sh(sys.executable, "-m", "pip", "install", "--quiet", "--no-input", *args)


def main() -> int:
    dest = sys.argv[1] if len(sys.argv) > 1 else "/tmp/rust"
    if os.path.isdir(os.path.join(dest, "prefix", "bin")):
        print(f"toolchain already present at {dest}")
    else:
        shutil.rmtree(STAGE, ignore_errors=True)
        os.makedirs(STAGE, exist_ok=True)
        pip("--no-deps", f"--target={STAGE}", *PARTS)
        try:
            import zstandard  # noqa: F401
        except ImportError:
            pip(f"--target={LIBS}", "zstandard")

        # The bundle is one zstd-compressed tar, cut into numbered parts.
        candidates: list[str] = []
        for pattern in ("part*", "data*", "*.zst", "*.zstd", "*.tar.zst", "*.bin"):
            candidates += glob.glob(os.path.join(STAGE, "**", pattern), recursive=True)
        candidates = sorted({c for c in candidates if os.path.isfile(c)})
        if not candidates:
            print("no toolchain parts found in the installed package", file=sys.stderr)
            return 1
        print(f"concatenating {len(candidates)} part(s)")
        blob = os.path.join(STAGE, "toolchain.tar.zst")
        with open(blob, "wb") as out:
            for part in candidates:
                with open(part, "rb") as src:
                    shutil.copyfileobj(src, out)

        sys.path.insert(0, LIBS)
        import zstandard  # type: ignore

        tar_path = os.path.join(STAGE, "toolchain.tar")
        with open(blob, "rb") as src, open(tar_path, "wb") as out:
            zstandard.ZstdDecompressor().copy_stream(src, out)
        os.makedirs(dest, exist_ok=True)
        with tarfile.open(tar_path) as tf:
            tf.extractall(dest)  # noqa: S202 — filter= is unsupported here
        os.remove(blob)
        os.remove(tar_path)

    for name in ("rustc", "cargo", "rustfmt", "cargo-clippy", "clippy-driver", "rust-analyzer"):
        binary = os.path.join(dest, "prefix", "bin", name)
        if os.path.exists(binary):
            os.chmod(binary, 0o755)
    version = sh(os.path.join(dest, "prefix", "bin", "cargo"), "--version", check=False)
    print(version.stdout.strip() or version.stderr.strip())
    print(f"export PATH={dest}/prefix/bin:$PATH")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
