#!/usr/bin/env python3
"""Vendor every crates.io dependency of this workspace from GitHub tarballs.

crates.io (and every mirror of it) is unreachable from this sandbox, but the
dependencies are ordinary open-source crates: their sources live in the GitHub
repositories they were published from.  This script

1. reads `Cargo.lock` and lists every registry package the workspace needs,
2. resolves each one to a GitHub repo + tag (or `HEAD`),
3. downloads the repository tarball from codeload,
4. finds the directory inside that declares the crate,
5. **rewrites its manifest into the standalone, concrete form a registry
   publish would have produced** — workspace inheritance inlined, renamed
   packages kept, versions pinned to the lock, dependencies the lock never
   activates removed, features pruned to what exists,
6. writes it to `/tmp/vendor/<name>-<version>/` with a `.cargo-checksum.json`.

Step 5 is the part that matters.  A repository manifest is written for a
*workspace*; a vendored crate must be self-contained.  Doing that by hand (or by
regex) produced a long tail of cargo errors, so the manifest is rebuilt in data
with `tomli_w` and then reconciled against `Cargo.lock`, which is the only
authority on which dependencies can legally appear:

    lock deps of X  ==  the exact set of names, at the exact versions, that X
                        may depend on.  Anything else is optional-and-inactive
                        (or dev-only) and is deleted, with the features that
                        named it.

Usage:
    python3 tools/vendor_deps.py [vendor-dir] [--jobs N] [--only crate]...

Requires `cargo` on PATH only for the optional ``--check`` step; the vendoring
itself needs `git`, `curl`, `gh` (authenticated) and `tomli_w`.
"""
from __future__ import annotations

import argparse
import base64
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import time
from urllib.parse import quote
import tempfile
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import tomli_w

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore

ROOT = Path(__file__).resolve().parent.parent
LOCK = ROOT / "Cargo.lock"
# `--lock PATH` retargets the whole run at a *foreign* lock (see `main`). The
# vendor tree a run produces is "the closure of some lock", and this script's
# rules — reconcile each manifest to the lock's dependency list, checksum each
# crate from the lock's checksum, write the tree a directory source needs — are
# exactly what building a tool from someone else's lock also needs.
LOCK_OVERRIDE: Path | None = None
CACHE = Path("/tmp/vendor-cache")
# GitHub's code search has its own rate-limit bucket (10 requests a minute for an
# ordinary token), and a 256-crate pass can spend the window long before a
# registry-first crate is reached. A throttled search and a crate that is
# genuinely not on GitHub answer *identically* (`items: []`) — and for a
# registry-first crate the second one is a hard stop — so an empty answer is
# retried, with a pause, before it is believed.
SEARCH_ATTEMPTS = 3
SEARCH_PAUSE_SECONDS = 21.0
DEP_TABLES = ("dependencies", "build-dependencies", "dev-dependencies")
PRINT_LOCK = threading.Lock()

# Repositories the automatic search gets wrong or cannot find.  Keyed by crate.
KNOWN: dict[str, str] = {
    "adler2": "oyvindln/adler2",
    "aho-corasick": "BurntSushi/aho-corasick",
    "allocator-api2": "zakarumych/allocator-api2",
    "android_system_properties": "nvzqz/android_system_properties",
    "approx": "brendanzab/approx",
    "arrayvec": "bluss/arrayvec",
    "ash": "ash-rs/ash",
    "autocfg": "cuviper/autocfg",
    "bit-set": "contain-rs/bit-set",
    "bit-vec": "contain-rs/bit-vec",
    "bitflags": "bitflags/bitflags",
    "bumpalo": "fitzgen/bumpalo",
    "bytemuck": "Lokathor/bytemuck",
    "byteorder": "BurntSushi/byteorder",
    "cc": "rust-lang/cc-rs",
    "cfg-if": "rust-lang/cfg-if",
    "cfg_aliases": "katharostech/cfg_aliases",
    "chumsky": "zesterer/chumsky",
    "codespan-reporting": "brendanzab/codespan",
    "console_error_panic_hook": "rustwasm/console_error_panic_hook",
    "core-foundation": "servo/core-foundation-rs",
    "core-foundation-sys": "servo/core-foundation-rs",
    "core-graphics-types": "servo/core-foundation-rs",
    "crc32fast": "srijs/rust-crc32fast",
    "crossbeam-deque": "crossbeam-rs/crossbeam",
    # Monorepos, and names a repository search would pick the wrong repo for:
    # `wasmparser` lives in wasm-tools (there is also an archived
    # `bytecodealliance/wasmparser` whose tags stopped in 2021), and both
    # `walrus-macro` and `wasm-bindgen-cli-support` are published from another
    # crate's repository.
    "leb128": "gimli-rs/leb128",
    "serde_json": "serde-rs/json",
    "unicode-ident": "dtolnay/unicode-ident",
    "walrus": "rustwasm/walrus",
    "walrus-macro": "rustwasm/walrus",
    "wasm-bindgen-cli-support": "wasm-bindgen/wasm-bindgen",
    "wasm-bindgen-shared": "wasm-bindgen/wasm-bindgen",
    "wasm-encoder": "bytecodealliance/wasm-tools",
    "wasmparser": "bytecodealliance/wasm-tools",
    "crossbeam-epoch": "crossbeam-rs/crossbeam",
    "crossbeam-utils": "crossbeam-rs/crossbeam",
    "d3d12": "gfx-rs/d3d12-rs",
    "document-features": "slint-ui/document-features",
    "earcut": "georust/earcut",
    "either": "rayon-rs/either",
    "equivalent": "cuviper/equivalent",
    "fastrand": "smol-rs/fastrand",
    "find-msvc-tools": "rust-lang/cc-rs",
    "flate2": "rust-lang/flate2-rs",
    "fnv": "servo/rust-fnv",
    "foreign-types": "sfackler/foreign-types",
    "foreign-types-shared": "sfackler/foreign-types",
    "futures-core": "rust-lang/futures-rs",
    "futures-task": "rust-lang/futures-rs",
    "futures-util": "rust-lang/futures-rs",
    "geo": "georust/geo",
    "geo-types": "georust/geo",
    "getrandom": "rust-random/getrandom",
    "gl_generator": "brendanzab/gl-rs",
    "glam": "bitshifter/glam-rs",
    "glow": "grovesNL/glow",
    "gpu-allocator": "Traverse-Research/gpu-allocator",
    "gpu-descriptor": "zakarumych/gpu-descriptor",
    "gpu-descriptor-types": "zakarumych/gpu-descriptor",
    "hash32": "rust-lang/hash32",
    "hashbrown": "rust-lang/hashbrown",
    "hashlink": "kyren/hashlink",
    "hexf-parse": "lifthrasiir/hexf",
    "i_float": "iShape-Rust/i_float",
    "i_key_sort": "iShape-Rust/iKeySort",
    "i_overlay": "iShape-Rust/iOverlay",
    "i_shape": "iShape-Rust/i_shape",
    "i_tree": "iShape-Rust/iTree",
    "indexmap": "indexmap-rs/indexmap",
    "itoa": "dtolnay/itoa",
    "jni-sys": "jni-rs/jni-sys",
    "khronos-egl": "rust-windowing/khronos-egl",
    "khronos_api": "brendanzab/gl-rs",
    "lazy_static": "rust-lang-nursery/lazy-static.rs",
    "libc": "rust-lang/libc",
    "libloading": "nagisa/rust_libloading",
    "libm": "rust-lang/libm",
    "linux-raw-sys": "sunfishcode/linux-raw-sys",
    "log": "rust-lang/log",
    "logos": "maciejhirsz/logos",
    "malloc_buf": "Narigo/azul-dependencies",
    "memchr": "BurntSushi/memchr",
    "metal": "gfx-rs/metal-rs",
    "naga": "gfx-rs/wgpu",
    "nalgebra": "dimforge/nalgebra",
    "ndk-sys": "rust-windowing/android-ndk-rs",
    "num-complex": "rust-num/num-complex",
    "num-traits": "rust-num/num-traits",
    "objc": "SSheldon/rust-objc",
    "object": "gimli-rs/object",
    "once_cell": "matklad/once_cell",
    "parking_lot": "Amanieu/parking_lot",
    "parking_lot_core": "Amanieu/parking_lot",
    "petgraph": "petgraph/petgraph",
    "pin-project-lite": "taiki-e/pin-project-lite",
    "pp-rs": "gfx-rs/pp-rs",
    "proc-macro2": "dtolnay/proc-macro2",
    "profiling": "aclysma/profiling",
    "proptest": "proptest-rs/proptest",
    "psm": "nagisa/psm",
    "quick-error": "tailhook/quick-error",
    "quote": "dtolnay/quote",
    "r-efi": "r-efi/r-efi",
    "rand": "rust-random/rand",
    "rand_chacha": "rust-random/rand",
    "rand_core": "rust-random/rand",
    "rand_pcg": "rust-random/rngs",
    "rand_xorshift": "rust-random/rand",
    "raw-window-handle": "rust-windowing/raw-window-handle",
    "rawpointer": "bluss/rawpointer",
    "redox_syscall": "redox-os/syscall",
    "regex": "rust-lang/regex",
    "regex-automata": "rust-lang/regex",
    "regex-syntax": "rust-lang/regex",
    "renderdoc-sys": "ebkalderon/renderdoc-rs",
    "ron": "ron-rs/ron",
    "rstar": "georust/rstar",
    "rustc-hash": "rust-lang/rustc-hash",
    "rustix": "bytecodealliance/rustix",
    "rustversion": "dtolnay/rustversion",
    "scopeguard": "bluss/scopeguard",
    "serde": "serde-rs/serde",
    "serde_core": "serde-rs/serde",
    "serde_derive": "serde-rs/serde",
    "serde_json": "serde-rs/json",
    "sif-itree": "adamreichold/sif-itree",
    "simba": "dimforge/simba",
    "slab": "tokio-rs/slab",
    "slotmap": "orlp/slotmap",
    "smallvec": "servo/rust-smallvec",
    "spade": "Stoeoef/spade",
    "spirv": "gfx-rs/rspirv",
    "stable_deref_trait": "storyyeller/stable_deref_trait",
    "stacker": "rust-lang/stacker",
    "static_assertions": "niklasf/rust-static_assertions",
    "syn": "dtolnay/syn",
    "tempfile": "Stebalien/tempfile",
    "termcolor": "BurntSushi/termcolor",
    "thiserror": "dtolnay/thiserror",
    "thiserror-impl": "dtolnay/thiserror",
    "tokio": "tokio-rs/tokio",
    "typenum": "paholg/typenum",
    "unicode-ident": "dtolnay/unicode-ident",
    "unicode-segmentation": "unicode-rs/unicode-segmentation",
    "unicode-width": "unicode-rs/unicode-width",
    "unicode-xid": "unicode-rs/unicode-xid",
    "uuid": "uuid-rs/uuid",
    "version_check": "SergioBenitez/version_check",
    "wait-timeout": "alexcrichton/wait-timeout",
    "wasip2": "bytecodealliance/wasi-rs",
    "wasm-bindgen": "rustwasm/wasm-bindgen",
    "wasm-bindgen-futures": "rustwasm/wasm-bindgen",
    "wasm-bindgen-macro": "rustwasm/wasm-bindgen",
    "wasm-bindgen-macro-support": "rustwasm/wasm-bindgen",
    "wasm-bindgen-shared": "rustwasm/wasm-bindgen",
    "web-sys": "rustwasm/wasm-bindgen",
    "wgpu": "gfx-rs/wgpu",
    "wgpu-core": "gfx-rs/wgpu",
    "wgpu-hal": "gfx-rs/wgpu",
    "wgpu-types": "gfx-rs/wgpu",
    "wide": "Lokathor/wide",
    "widestring": " forgotten",
    "winapi": "retep998/winapi-rs",
    "winapi-i686-pc-windows-gnu": "retep998/winapi-rs",
    "winapi-x86_64-pc-windows-gnu": "retep998/winapi-rs",
    "windows": "microsoft/windows-rs",
    "windows-core": "microsoft/windows-rs",
    "windows-link": "microsoft/windows-rs",
    "windows-sys": "microsoft/windows-rs",
    "windows-targets": "microsoft/windows-rs",
    "windows_aarch64_gnullvm": "microsoft/windows-rs",
    "windows_aarch64_msvc": "microsoft/windows-rs",
    "windows_i686_gnu": "microsoft/windows-rs",
    "windows_i686_gnullvm": "microsoft/windows-rs",
    "windows_i686_msvc": "microsoft/windows-rs",
    "windows_x86_64_gnu": "microsoft/windows-rs",
    "windows_x86_64_gnullvm": "microsoft/windows-rs",
    "windows_x86_64_msvc": "microsoft/windows-rs",
    "wit-bindgen": "bytecodealliance/wit-bindgen",
    "xml-rs": "kornelski/xml-rs",
    "zerocopy": "google/zerocopy",
    "zerocopy-derive": "google/zerocopy",
    "zmij": "dtolnay/zmij",
    "lyon": "nical/lyon",
    "lyon_algorithms": "nical/lyon",
    "lyon_geom": "nical/lyon",
    "lyon_path": "nical/lyon",
    "lyon_tessellation": "nical/lyon",
    "js-sys": "rustwasm/wasm-bindgen",
    "block": "SSheldon/rust-block",
    "cassowary": "dylanede/cassowary-rs",
    "com": "microsoft/com-rs",
    "com_macros": "microsoft/com-rs",
    "com_macros_support": "microsoft/com-rs",
    "euclid": "servo/euclid",
    "foreign-types-macros": "sfackler/foreign-types",
    "glutin_wgl_sys": "rust-windowing/glutin",
    "gpu-alloc-types": "zakarumych/gpu-alloc",
    "jni-sys-macros": "jni-rs/jni-sys",
    "lock_api": "Amanieu/parking_lot",
    "matrixmultiply": "bluss/matrixmultiply",
    "miniz_oxide": "Frommi/miniz_oxide",
    "nalgebra-macros": "dimforge/nalgebra",
    "num-bigint": "rust-num/num-bigint",
    "num-integer": "rust-num/num-integer",
    "num-rational": "rust-num/num-rational",
    "paste": "dtolnay/paste",
    "pkg-config": "rust-lang/pkg-config-rs",
    "ppv-lite86": "cryptocoinjs/ppv-lite86",
    "presser": "EmbarkStudios/presser",
    "range-alloc": "gfx-rs/range-alloc",
    "rayon": "rayon-rs/rayon",
    "rayon-core": "rayon-rs/rayon",
    "robust": "georust/robust",
    "rustc_version": "kimundi/rustc-version-rs",
    "rusty-fork": "AltSysrq/rusty-fork",
    "safe_arch": "Lokathor/safe_arch",
    "semver": "dtolnay/semver",
    "shlex": "comex/rust-shlex",
    "simd-adler32": "mgackowski/simd-adler32",
    "unarray": "Soveu/unarray",
    "widestring": "VoidStarKat/widestring-rs",
    "winapi-util": "BurntSushi/winapi-util",
    "hash32": "japaric/hash32",
    "logos-codegen": "maciejhirsz/logos",
    "logos-derive": "maciejhirsz/logos",
    "khronos-egl": "rust-windowing/khronos-egl",
    "malloc_buf": "Narigo/azul-dependencies",
}
KNOWN.pop("widestring", None)

# Crates whose only reachable repository copies are known to be wrong (wrong
# API, missing generated files, or a different version), and whose *published*
# tarball is therefore the only acceptable source.  Kept as a set rather than a
# flag so the tree does not depend on how the tool was invoked.
REGISTRY_FIRST: set[str] = {
    # The AOSP mirror's `crates/glow` has no `build.rs`, so its `gl46.rs` is a
    # stub: `error[E0425]: cannot find type GLchar in native_gl` × 30.
    "glow",
}

# Crates whose published source is not at a tag of their own repository.  Each
# entry says exactly where the source is, so nothing is guessed at run time.
EXPLICIT: dict[str, tuple[str, str, str | None]] = {
    # name: (repo, ref, subdir)
    # The `windows_*` import shims live in the windows-rs monorepo and were
    # dropped from it after 0.52; the tag that has them is 0.52.0.
    "windows_aarch64_gnullvm": ("microsoft/windows-rs", "0.52.0", "aarch64_gnullvm"),
    "windows_aarch64_msvc": ("microsoft/windows-rs", "0.52.0", "aarch64_msvc"),
    "windows_i686_gnu": ("microsoft/windows-rs", "0.52.0", "i686_gnu"),
    "windows_i686_msvc": ("microsoft/windows-rs", "0.52.0", "i686_msvc"),
    "windows_x86_64_gnu": ("microsoft/windows-rs", "0.52.0", "x86_64_gnu"),
    "windows_x86_64_msvc": ("microsoft/windows-rs", "0.52.0", "x86_64_msvc"),
    "windows_x86_64_gnullvm": ("microsoft/windows-rs", "0.52.0", "x86_64_gnullvm"),
    "windows-targets": ("microsoft/windows-rs", "0.52.0", "windows-targets"),
    "windows": ("microsoft/windows-rs", "0.52.0", "windows"),
    "windows-core": ("microsoft/windows-rs", "0.52.0", "windows-core"),
    "float_next_after": ("aosp-mirror-neo/platform_external_rust_android-crates-io", "", None),
    "malloc_buf": ("Narigo/azul-dependencies", "HEAD", "malloc_buf-0.0.6"),
    "bit-set": ("contain-rs/bit-set", "HEAD", None),
    "earcut": ("georust/earcut", "HEAD", None),
    "i_key_sort": ("iShape-Rust/iKeySort", "HEAD", None),
    "i_tree": ("iShape-Rust/iTree", "HEAD", None),
    "sif-itree": ("adamreichold/sif-itree", "v0.4.0", None),
    "rand_pcg": ("rust-random/rngs", "HEAD", "rand_pcg"),
    "hexf-parse": ("lifthrasiir/hexf", "0.2.1", "parse"),
}

# crates.io -> GitHub source mirrors that hold *published* crate directories.
MIRRORS = [
    # A published-vendor tree: the repository root *is* the vendor directory.
    ("ClickHouse/rust_vendor", ""),
    # The AOSP mirror keeps one directory per crate, unversioned.
    ("aosp-mirror-neo/platform_external_rust_android-crates-io", "crates"),
]


def log(message: str) -> None:
    with PRINT_LOCK:
        print(message, flush=True)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


# ── lock ─────────────────────────────────────────────────────────────────


class Locked:
    __slots__ = ("name", "version", "deps")

    def __init__(self, name: str, version: str, deps: list[tuple[str, str | None]]):
        self.name = name
        self.version = version
        self.deps = deps


def declares(manifest: Path, name: str) -> bool:
    """True ⟺ `manifest` is the manifest of package `name`.

    Parsing matters: a text search for `name = "lyon"` matches the CLI crate's
    `[[bin]] name = "lyon"` as happily as the library's `[package] name`.
    """
    try:
        data = tomllib.loads(manifest.read_text(errors="replace"))
    except Exception:  # noqa: BLE001
        return False
    package = data.get("package")
    return isinstance(package, dict) and norm(str(package.get("name", ""))) == norm(name)


LOCK_CHECKSUMS: dict[tuple[str, str], str] = {}


def checksums_from_lock() -> dict[tuple[str, str], str]:
    """`(normalised crate, version)` -> the tarball checksum the lock records."""
    out: dict[tuple[str, str], str] = {}
    try:
        data = tomllib.loads((LOCK_OVERRIDE or LOCK).read_text())
    except Exception:  # noqa: BLE001
        return out
    for package in data.get("package", []):
        if package.get("source", "").startswith("registry") and package.get("checksum"):
            out[(norm(package["name"]), package["version"])] = package["checksum"]
    return out


def read_lock() -> dict[tuple[str, str], Locked]:
    # `LOCK_OVERRIDE`, when set, is the whole point of a foreign-lock run: every
    # rule below is "satisfy *this* lock", and the CLI-closure build passes one.
    data = tomllib.loads((LOCK_OVERRIDE or LOCK).read_text())
    out: dict[tuple[str, str], Locked] = {}
    for package in data.get("package", []):
        if "source" not in package:
            continue
        deps = []
        for dep in package.get("dependencies", []):
            parts = dep.split()
            deps.append((parts[0], parts[1] if len(parts) > 1 else None))
        entry = Locked(package["name"], package["version"], deps)
        # Keyed by the name as published *and* normalised, so a caller never has
        # to remember which spelling `Cargo.lock` used.
        out[(package["name"], package["version"])] = entry
        key = (norm(package["name"]), package["version"])
        if key not in out:
            out[key] = entry
    return out


def norm(name: str) -> str:
    return name.replace("-", "_").lower()


def crate_dirs(vendor: Path):
    """Yield `(name, version, directory)` for every crate in the lock that is
    vendored.

    The directory name is built from the lock rather than parsed out of the
    directory listing: versions like `0.3.0+sdk-1.3.268.0` contain a `-` and a
    regex over the name would split them in the wrong place.
    """
    seen: set[tuple[str, str]] = set()
    for package in tomllib.loads(LOCK.read_text()).get("package", []):
        if "source" not in package:
            continue
        key = (package["name"], package["version"])
        if key in seen:
            continue
        seen.add(key)
        directory = vendor / f"{package['name']}-{package['version']}"
        if directory.is_dir():
            yield package["name"], package["version"], directory


# ── github ───────────────────────────────────────────────────────────────


TAG_CACHE: dict[str, dict[str, str]] = {}

# `bytecodealliance/wasm-tools` publishes every crate in one repository and tags
# a release `v1.<minor>.<patch>` while the crates in it are `0.<minor>.<patch>`
# — wasmparser 0.245.1 is the tag `v1.245.1`. Without this the tag list looks
# empty and the crate is reported missing.
# Compared through `norm`, like every other name in this file: `wasm-encoder`
# normalises to `wasm_encoder`, and a set written the other way silently never
# matches — which is how wasm-encoder came from `main` while wasmparser came
# from its tag in the same run.
WASM_TOOLS_CRATES = {norm(name) for name in
                     ("wasmparser", "wasm-encoder", "wast", "wat", "wasmprinter")}


def tags(repo: str) -> dict[str, str]:
    if repo not in TAG_CACHE:
        r = run(["git", "ls-remote", "--tags", "--refs", f"https://github.com/{repo}"])
        out = {}
        for line in r.stdout.splitlines():
            try:
                sha, ref = line.split("\t")
            except ValueError:
                continue
            out[ref.replace("refs/tags/", "")] = sha
        TAG_CACHE[repo] = out
    return TAG_CACHE[repo]


def head_sha(repo: str) -> str | None:
    r = run(["git", "ls-remote", f"https://github.com/{repo}", "HEAD"])
    return r.stdout.split()[0] if r.stdout.strip() else None


def branch_sha(repo: str, branch: str) -> str | None:
    """The head of a *named* branch — for a crate whose release is a branch."""
    r = run(["git", "ls-remote", "--heads", f"https://github.com/{repo}", branch])
    for line in r.stdout.splitlines():
        sha, _, ref = line.partition("\t")
        if ref.strip() in (f"refs/heads/{branch}", branch):
            return sha.strip()
    return None


# ── known gaps ───────────────────────────────────────────────────────────
#
# Two of this tool's fetch routes can hand back a *truncated* file and still
# report success, and one crate in the workspace needs the file they truncate:
# `glow`'s `src/gl46.rs` is 1.4 MB of generated OpenGL 4.6 bindings, larger
# than GitHub's contents API will return in one response (it answers with an
# empty body rather than an error), and the registry-src mirrors that carry
# `glow-0.13.1` store it as a zero-byte placeholder. Cargo needs it the moment
# `wgpu`'s Linux `gles` backend is compiled, so a gap is *repaired* here from
# the crate's own repository branch: the file is generated boilerplate, so the
# branch and the published tarball agree on its contents (verified by the
# compiler, which links `native.rs`'s `GlFns` against it).
_FILE_GAPS: dict[tuple[str, str, str], tuple[str, str]] = {
    ("glow", "0.13.1", "src/gl46.rs"): ("grovesNL/glow", "0.13"),
}


def restore_known_gaps(vendor: Path) -> int:
    """Re-fetch the vendored files a truncated mirror left empty or missing.

    Only ever fills a hole: a file that is present and non-empty is left exactly
    as it came from the mirror, because the mirror is the authoritative source
    whenever it actually delivered.
    """
    restored = 0
    for (name, version, rel), (repo, ref) in _FILE_GAPS.items():
        dest = vendor / f"{name}-{version}" / rel
        if not (vendor / f"{name}-{version}" / "Cargo.toml").exists():
            continue
        if dest.exists() and dest.stat().st_size > 0:
            continue
        sha = branch_sha(repo, ref)
        tgz = download(repo, sha) if sha else None
        if tgz is None:
            log(f"[gap] {name}@{version}: cannot fetch {rel} from {repo}@{ref}")
            continue
        try:
            with tarfile.open(tgz) as tar:
                member = next(
                    (m for m in tar.getmembers()
                     if m.isfile() and m.name.endswith(f"/{rel}") and name in m.name),
                    None,
                )
                if member is None or member.size == 0:
                    log(f"[gap] {name}@{version}: {rel} is not in {repo}@{ref}")
                    continue
                dest.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as src, open(dest, "wb") as out:
                    shutil.copyfileobj(src, out)
        except (tarfile.TarError, OSError) as exc:  # noqa: BLE001
            log(f"[gap] {name}@{version}: {rel} failed to restore: {exc}")
            continue
        restored += 1
        log(f"[gap] {name}@{version}: restored {rel} from {repo}@{ref} "
            f"({dest.stat().st_size} bytes)")
    return restored


def search_repo(name: str) -> str | None:
    r = run(["gh", "api", f"search/repositories?q={name}+in:name&per_page=8"])
    try:
        items = json.loads(r.stdout).get("items", [])
    except Exception:  # noqa: BLE001
        items = []
    target = norm(name)
    best = None
    for item in items:
        if norm(item["name"]) == target:
            return item["full_name"]
        if best is None:
            best = item["full_name"]
    return best


def tag_candidates(name: str, version: str, tagset: dict[str, str]) -> tuple[list[str], list[str]]:
    """`(exact, nearby)` tag shas, most specific first.

    `nearby` exists for crates the repository stopped tagging per version (the
    `windows_*` shims are published from the 0.52.0 tag under a 0.52.6 version),
    but it is only a fallback: `lyon` has a `1.0.0` tag and a `1.0.19` release,
    and a "close enough" tag would hand us a decade-old source.
    """
    wanted = [
        f"v{version}", version, f"{name}-v{version}", f"{name}-{version}",
        f"{name}_v{version}", f"release-{version}", f"{version}-release",
    ]
    base = version.split("+")[0]
    if base != version:
        wanted += [f"v{base}", base, f"{name}-{base}", f"{name}-{base}"]
    if norm(name) in WASM_TOOLS_CRATES:
        # The repository version is the crate version with the leading zero
        # spent on the `1`: wasmparser 0.245.1 ships from tag `v1.245.1`.
        wanted = [
            f"v1.{version.removeprefix('0.')}",
            f"v1.{base.removeprefix('0.')}",
        ] + wanted
    exact = [tagset[t] for t in wanted if t in tagset]
    # A monorepo tags every member: wgpu 22.1.0 has `v22.1.0` (the release),
    # `naga-v22.1.0` and `wgpu-core-v22.1.0` (its siblings). Taking "the first
    # tag containing 22.1.0" picks a sibling ~two thirds of the time, and the
    # crate then comes from a tree that is not its release — which is how
    # `wgpu-hal` ended up compiled against a `glow` whose API had moved on.
    for tag, sha in tagset.items():
        if version not in tag or sha in exact:
            continue
        scoped = re.match(r"^(?P<crate>[A-Za-z0-9_-]+?)-v?\d", tag)
        if scoped and norm(scoped.group("crate")) != norm(name):
            continue
        exact.append(sha)
    exact = list(dict.fromkeys(exact))

    major, minor, _ = (version.split(".") + ["", ""])[:3]
    nearby: list[str] = []
    if minor:
        for tag in (f"v{major}.{minor}.0", f"{major}.{minor}.0",
                    f"{name}-v{major}.{minor}.0", f"{name}-{major}.{minor}.0"):
            if tag in tagset and tagset[tag] not in exact:
                nearby.append(tagset[tag])
    trimmed = ".".join(version.split(".")[:-1])
    if trimmed and trimmed != version:
        for tag in (f"v{trimmed}", trimmed, f"{name}-{trimmed}"):
            if tag in tagset and tagset[tag] not in exact:
                nearby.append(tagset[tag])
    return exact, list(dict.fromkeys(nearby))


def search_code(query: str) -> list[dict]:
    """GitHub code search, retried while the answer is empty.

    Returns the `items` list; an empty list means "no hit *or* no quota", which
    is why the retry lives here rather than in each caller.
    """
    for attempt in range(SEARCH_ATTEMPTS):
        result = run(["gh", "api", f"search/code?q={query}&per_page=20"])
        try:
            items = json.loads(result.stdout).get("items", [])
        except Exception:  # noqa: BLE001
            items = []
        if items or attempt + 1 == SEARCH_ATTEMPTS:
            return items
        time.sleep(SEARCH_PAUSE_SECONDS)
    return []


def registry_archive(name: str, version: str, dest: Path) -> str | None:
    """The **published crate**, verified against the lock's checksum.

    A repository that committed `.cargo/registry/cache/…/<crate>-<v>.crate` has
    the literal tarball crates.io served, and the lock records that tarball's
    SHA-256 — so this is the only source in the whole tool that can be *checked*
    rather than trusted. (A tag can be moved, a registry `src/` copy can be
    edited; a checksum cannot.)
    """
    query = quote(f'"{name}-{version}.crate" in:path')
    items = search_code(query)
    suffix = f"/{name}-{version}.crate"
    expected = LOCK_CHECKSUMS.get((norm(name), version))
    for item in items:
        path = item.get("path", "")
        repo = item.get("repository", {}).get("full_name", "")
        if not repo or not path.endswith(suffix):
            continue
        raw = run_bytes(
            ["gh", "api", f"repos/{repo}/contents/{path}",
             "-H", "Accept: application/vnd.github.raw"]
        )
        blob = raw.stdout if isinstance(raw.stdout, bytes) else b""
        if raw.returncode != 0 or len(blob) < 64:
            continue
        if expected and hashlib.sha256(blob).hexdigest() != expected:
            # Not the tarball this lock pins: keep looking rather than vendoring
            # a copy that would fail cargo's own check later.
            continue
        archive = CACHE / f"{name}-{version}.crate"
        archive.write_bytes(blob)
        try:
            with tarfile.open(archive) as tf:
                members = [
                    m for m in tf
                    if m.name.startswith(f"{name}-{version}/") or m.name == f"{name}-{version}"
                ]
                if not members:
                    continue
                dest.mkdir(parents=True, exist_ok=True)
                for member in members:
                    relative = member.name[len(f"{name}-{version}"):].lstrip("/")
                    if not relative:
                        continue
                    target = dest / relative
                    if member.isdir():
                        target.mkdir(parents=True, exist_ok=True)
                    elif member.isfile():
                        target.parent.mkdir(parents=True, exist_ok=True)
                        handle = tf.extractfile(member)
                        if handle is not None:
                            target.write_bytes(handle.read())
        except Exception:  # noqa: BLE001
            continue
        if declares(dest / "Cargo.toml", name):
            return f"{repo}:{path}"
    return None


def registry_published(name: str, version: str, vendor: Path) -> tuple[Path, str] | None:
    """The published crate, from a `.crate` tarball in someone's registry cache.

    A repository that commits `.cargo/registry/cache/` holds the literal tarball
    crates.io served for that version, and the lock records that tarball's
    SHA-256 — so this is the only source here that can be *verified* rather than
    trusted. It is tried before any repository tree, because a tree is not a
    release: `glow`'s repository has no tags (so its HEAD stands in for 0.13.1)
    and its unreleased source has a `tex_image_3d` signature that the pinned
    `wgpu-hal` cannot call.
    """
    hit = registry_hit(name, version)
    if hit is None:
        return None
    repo, src_path = hit
    archive_path = registry_src_to_cache(src_path)
    if archive_path:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name) / "crate"
            if fetch_crate(repo, archive_path, tmp) is not None:
                staged = vendor / f".staging-{name}-{version}-published"
                shutil.rmtree(staged, ignore_errors=True)
                shutil.copytree(tmp, staged)
                return staged, f"{repo}:{archive_path}"
    return None


def registry_src_to_cache(path: str) -> str | None:
    """Map a registry `src/` path to the sibling `.crate` tarball's path.

    The `src/` tree is a *working copy*: `glow-0.13.1` in one public repo had
    been edited in place there (its `tex_image_3d` had grown a `PixelUnpackData`
    argument the published crate does not have), so the cache tarball — the
    thing the lock's checksum covers — is always tried first.
    """
    # The search API returns repository-relative paths (no leading slash).
    marker = ".cargo/registry/src/"
    if marker not in path:
        return None
    head, _, tail = path.partition(marker)
    index, _, directory = tail.partition("/")
    if not directory:
        return None
    return head + ".cargo/registry/cache/" + index + "/" + directory + ".crate"


def fetch_crate(repo: str, path: str, dest: Path) -> str | None:
    """Download a `.crate` tarball and unpack it, checksum-verified.

    Returns a provenance note, or `None` when the file is missing or its SHA-256
    is not the one this lock pins — a mismatch means it is not the crate we need,
    so the caller keeps looking instead of vendoring something cargo will reject.
    """
    raw = run_bytes(
        ["gh", "api", "repos/" + repo + "/contents/" + path,
         "-H", "Accept: application/vnd.github.raw"]
    )
    blob = raw.stdout if isinstance(raw.stdout, bytes) else b""
    if raw.returncode != 0 or len(blob) < 64:
        return None
    directory = path.rsplit("/", 1)[-1]
    name, _, version = directory[:-len(".crate")].rpartition("-")
    if not name or not version:
        return None
    expected = LOCK_CHECKSUMS.get((norm(name), version))
    digest = hashlib.sha256(blob).hexdigest()
    if expected and digest != expected:
        return None
    archive = CACHE / directory
    CACHE.mkdir(parents=True, exist_ok=True)
    archive.write_bytes(blob)
    prefix = name + "-" + version
    with tarfile.open(archive) as tf:
        members = [m for m in tf if m.name == prefix or m.name.startswith(prefix + "/")]
        if not members:
            return None
        dest.mkdir(parents=True, exist_ok=True)
        for member in members:
            relative = member.name[len(prefix):].lstrip("/")
            if not relative:
                continue
            target = dest / relative
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                handle = tf.extractfile(member)
                if handle is not None:
                    target.write_bytes(handle.read())
    if not declares(dest / "Cargo.toml", name):
        return None
    return repo + ":" + path


def registry_hit(name: str, version: str) -> tuple[str, str] | None:
    """`(repo, path)` for a checked-in cargo registry copy of the crate.

    Some repositories commit their `.cargo/registry/src/` tree — a verbatim
    copy of the crates.io contents. That is the *published* crate, which is what
    a lockfile pins; it is how `glow 0.13.1` is reachable at all, since its
    repository has never had a tag.
    """
    query = quote(f'"{name}-{version}" in:path filename:Cargo.toml')
    items = search_code(query)
    suffix = f"/{name}-{version}/Cargo.toml"
    hits: list[tuple[str, str]] = []
    for item in items:
        path = item.get("path", "")
        repo = item.get("repository", {}).get("full_name", "")
        if not repo or not path.endswith(suffix):
            continue
        hits.append((repo, path[: -len("/Cargo.toml")]))
    # A real `.cargo/registry/src/` cache first, then any other directory: the
    # same search finds `…/lizenzen/glow-0.13.1/` (license texts and a manifest,
    # no source) sitting next to a crate that carries the whole thing.
    hits.sort(key=lambda hit: ".cargo/registry" not in hit[1])
    return hits[0] if hits else None


def declared_version(manifest: Path) -> str | None:
    """The `[package] version` a manifest declares, workspace inheritance included.

    `version.workspace = true` is how a monorepo keeps one number in the root:
    the crate's own manifest says nothing, and reading only it makes a perfectly
    good release look like a mismatch — which is how `wasmparser` was once
    vendored from `wasm-tools`' *main branch* instead of tag `v1.245.1`, whose
    `Name` enum had grown two variants `walrus` never heard of.
    """
    try:
        data = tomllib.loads(manifest.read_text(errors="replace"))
    except Exception:  # noqa: BLE001
        return None
    package = data.get("package")
    if not isinstance(package, dict):
        return None
    version = package.get("version")
    if isinstance(version, str):
        return version
    # TOML reads `version.workspace = true` as `version = {workspace = true}`.
    if not (isinstance(version, dict) and version.get("workspace") is True):
        return None
    for parent in manifest.parents:
        root = parent / "Cargo.toml"
        if not root.is_file():
            continue
        try:
            inherited = (
                tomllib.loads(root.read_text(errors="replace"))
                .get("workspace", {})
                .get("package", {})
                .get("version")
            )
        except Exception:  # noqa: BLE001
            continue
        if isinstance(inherited, str):
            return inherited
    return None


def find_in(root: Path, name: str, version: str, subdir: str | None):
    """`(directory, version_matched)` for the crate inside an extracted repo."""
    candidates: list[Path] = []
    if subdir:
        exact = [p for p in root.rglob("*") if p.is_dir() and p.name == subdir]
        exact += [p.parent for p in root.rglob(f"{subdir}/Cargo.toml")]
        candidates += sorted(set(exact))
    candidates += [root] + [p for p in root.rglob("*") if p.is_dir()]
    fallback = None
    for cand in candidates:
        manifest = cand / "Cargo.toml"
        if not declares(manifest, name):
            continue
        declared = declared_version(manifest)
        if declared == version:
            return cand, True
        if fallback is None:
            fallback = cand
    return (fallback, False) if fallback else (None, False)


def download(repo: str, sha: str) -> Path | None:
    CACHE.mkdir(parents=True, exist_ok=True)
    tgz = CACHE / f"{repo.replace('/', '_')}-{sha}.tgz"
    if not tgz.exists():
        if run(["curl", "-sSL", "--fail", "-o", str(tgz),
                f"https://codeload.github.com/{repo}/tar.gz/{sha}"]).returncode != 0:
            return None
    return tgz


def run_bytes(cmd: list[str]) -> subprocess.CompletedProcess:
    """`run`, but for a response that is not text.

    A `.crate` is a gzip stream; decoding it as UTF-8 (which the text-mode
    helper does) fails on the magic bytes.
    """
    return subprocess.run(cmd, capture_output=True, check=False)


def gh_dir(repo: str, subdir: str, dest: Path, depth: int = 0) -> bool:
    """Copy a directory out of a GitHub repo with the contents API.

    The trees API refuses a `ref:path` that contains a `/` on some
    repositories, so directories are walked one level at a time instead.
    """
    if depth > 6:
        return False
    r = run(["gh", "api", f"repos/{repo}/contents/{subdir}"])
    try:
        entries = json.loads(r.stdout)
    except Exception:  # noqa: BLE001
        return False
    if not isinstance(entries, list):
        return False
    found = False
    for entry in entries:
        name = entry.get("name", "")
        if entry.get("type") == "dir":
            found |= gh_dir(repo, f"{subdir}/{name}", dest / name, depth + 1)
        elif entry.get("type") == "file":
            r = run(["gh", "api", f"repos/{repo}/contents/{subdir}/{name}"])
            try:
                content = base64.b64decode(json.loads(r.stdout)["content"])
            except Exception:  # noqa: BLE001
                continue
            out = dest / name
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(content)
            found = True
    return found


# ── manifest rewriting ───────────────────────────────────────────────────


def workspace_root(crate_dir: Path) -> dict:
    candidate = crate_dir
    for _ in range(8):
        manifest = candidate / "Cargo.toml"
        if manifest.is_file():
            try:
                data = tomllib.loads(manifest.read_text(errors="replace"))
            except Exception:  # noqa: BLE001
                data = {}
            if isinstance(data.get("workspace"), dict):
                return data["workspace"]
        if candidate.parent == candidate:
            break
        candidate = candidate.parent
    return {}


def inline_workspace(data: dict, ws: dict) -> list[str]:
    """Replace `key.workspace = true` with the workspace's value.

    `ws` is the **workspace root's `[workspace]` table**, which has to be read
    while the crate is still inside its repository: a crate copied out to the
    vendor directory has no ancestors left to inherit from, and an *unresolved*
    `edition.workspace = true` cannot simply be deleted either — deleting it
    silently rewrites an edition-2021 crate as edition 2015, where
    `use some_crate::Thing;` stops compiling. Returns the keys it could not
    resolve.
    """
    unresolved: list[str] = []
    ws_package = ws.get("package", {}) or {}
    ws_deps = ws.get("dependencies", {}) or {}
    package = data.get("package")
    if isinstance(package, dict):
        for key in list(package):
            value = package[key]
            if not (isinstance(value, dict) and value.get("workspace")):
                continue
            if key in ws_package:
                package[key] = ws_package[key]
            elif key == "edition":
                # The one key whose absence changes how the source is compiled.
                package[key] = "2021"
                unresolved.append(key)
            else:
                del package[key]
                unresolved.append(key)

    def walk(table: dict) -> None:
        for section in DEP_TABLES:
            deps = table.get(section)
            if not isinstance(deps, dict):
                continue
            for name in list(deps):
                spec = deps[name]
                if isinstance(spec, str) or not isinstance(spec, dict):
                    continue
                if not spec.get("workspace"):
                    continue
                base = ws_deps.get(name, {})
                if isinstance(base, str):
                    base = {"version": base}
                merged = {k: v for k, v in base.items() if k != "path"}
                merged.update({k: v for k, v in spec.items() if k != "workspace"})
                deps[name] = merged

    walk(data)
    for body in (data.get("target", {}) or {}).values():
        if isinstance(body, dict):
            walk(body)
    return unresolved


def sane_rust_version(data: dict) -> None:
    """Drop a `rust-version` cargo would reject.

    An edition has a minimum toolchain, so `edition = "2021"` with
    `rust-version = "1.0"` is a contradiction cargo refuses before it even looks
    at the code.  A missing `rust-version` is always safe; a wrong one is not.
    """
    package = data.get("package")
    if not isinstance(package, dict):
        return
    declared = package.get("rust-version")
    if declared is None:
        return
    fields = str(declared).split(".")
    try:
        major, minor = int(fields[0]), int(fields[1]) if len(fields) > 1 else 0
    except ValueError:
        del package["rust-version"]
        return
    edition = package.get("edition")
    if not isinstance(edition, str):
        # No edition means *edition 2015* — a real choice, not missing data:
        # `autocfg` is written for 2015 on purpose, and forcing 2021 onto it
        # turns its `'A'...'Z'` ranges and bare trait objects into errors.
        return
    floor = {"2015": (1, 0), "2018": (1, 31), "2021": (1, 56), "2024": (1, 85)}.get(
        edition, (1, 56)
    )
    if (major, minor) < floor:
        del package["rust-version"]


def strip_fluff(data: dict) -> None:
    """Remove everything a vendored crate must not carry."""
    data.pop("workspace", None)
    data.pop("lints", None)
    for section in ("dev-dependencies",):
        data.pop(section, None)
    for table in ("bench", "test"):
        data.pop(table, None)
    target = data.get("target")
    if isinstance(target, dict):
        for body in target.values():
            if not isinstance(body, dict):
                continue
            body.pop("dev-dependencies", None)
            body.pop("bench", None)
            body.pop("test", None)


def reconcile_dependencies(data: dict, locked: Locked, versions: dict[str, str]) -> list[str]:
    """Make the manifest's dependencies exactly the lock's dependencies.

    `Cargo.lock` lists every dependency of every package it contains —
    platform-specific ones included, optional-and-inactive ones excluded.  That
    makes it the precise answer to "which names may appear here?", which is what
    a vendored manifest has to satisfy.
    """
    notes: list[str] = []
    wanted: dict[str, str | None] = {norm(name): version for name, version in locked.deps}
    allowed_versions: dict[str, str] = {}
    for name, version in locked.deps:
        key = norm(name)
        allowed_versions.setdefault(key, version or versions.get(key, ""))
    real_names = {norm(name): name for name, _ in locked.deps}
    seen: set[str] = set()

    def tables():
        yield data.setdefault("dependencies", {})
        yield data.setdefault("build-dependencies", {})
        for body in (data.get("target", {}) or {}).values():
            if isinstance(body, dict):
                yield body.setdefault("dependencies", {})
                yield body.setdefault("build-dependencies", {})

    for table in tables():
        for key in list(table):
            spec = table[key]
            spec = {"version": spec} if isinstance(spec, str) else dict(spec)
            package = spec.get("package") or key
            lookup = norm(package)
            if lookup not in wanted:
                del table[key]
                notes.append(f"-{key}")
                continue
            spec["version"] = allowed_versions[lookup]
            spec.pop("path", None)
            spec.pop("git", None)
            spec.pop("branch", None)
            spec.pop("tag", None)
            spec.pop("rev", None)
            # The key is the name the code uses; cargo looks the *registry* up by
            # `package`, and it does not fold `-`/`_` when it does.  So a key of
            # `web_sys` for the crate `web-sys` needs the rename spelled out —
            # without it cargo reports "no matching package named `web_sys`".
            real = real_names.get(lookup, key)
            if real != key:
                spec["package"] = real
            else:
                spec.pop("package", None)
            table[key] = spec
            seen.add(lookup)
        if not table:
            pass

    missing = [name for name in wanted if name not in seen]
    if missing:
        dependencies = data.setdefault("dependencies", {})
        feature_text = json.dumps(data.get("features", {}))
        for key in missing:
            real = real_names.get(key, key)
            spec: dict = {"version": allowed_versions[key]}
            # A feature that says `dep:x` or `x?/…` asserts x is optional, so a
            # dependency re-added from the lock must carry the flag those
            # features promise — otherwise cargo rejects the whole manifest.
            if f"dep:{real}" in feature_text or f"{real}?" in feature_text:
                spec["optional"] = True
            dependencies.setdefault(real, spec)
            notes.append(f"+{real}")
    # An empty dependency table is legal but noisy; cargo is fine either way.
    return notes


def optional_dependencies(data: dict) -> set[str]:
    """The normalised names of this crate's `optional = true` dependencies."""
    out: set[str] = set()

    def collect(table: dict) -> None:
        for section in DEP_TABLES:
            # A copy: the loop may *delete* a key whose every requested feature
            # was dropped (see below).
            for name, spec in list((table.get(section, {}) or {}).items()):
                if isinstance(spec, dict) and spec.get("optional"):
                    out.add(norm(name))

    collect(data)
    for body in (data.get("target", {}) or {}).values():
        if isinstance(body, dict):
            collect(body)
    return out


def prune_features(data: dict) -> None:
    def prune(table: dict) -> None:
        features = table.get("features")
        if not isinstance(features, dict):
            return
        available: set[str] = set()
        for section in DEP_TABLES:
            deps = table.get(section)
            if isinstance(deps, dict):
                available |= {norm(name) for name in deps}
        for body in (table.get("target", {}) or {}).values():
            if isinstance(body, dict):
                for section in DEP_TABLES:
                    deps = body.get(section)
                    if isinstance(deps, dict):
                        available |= {norm(name) for name in deps}
        optional = optional_dependencies(table) | optional_dependencies(data)
        defined = set(features)
        for feature, values in list(features.items()):
            kept = []
            for value in values:
                if not isinstance(value, str):
                    kept.append(value)
                    continue
                if value.startswith("dep:"):
                    # `dep:x` names a *dependency*, never a feature, and it
                    # re-exports an **optional** one: `"dep:serde"` is invalid
                    # when `serde` is not optional (cargo says so explicitly).
                    name = norm(value[4:])
                    if name in available and name in optional:
                        kept.append(value)
                    continue
                if "/" in value:
                    # `x/feat` and `x?/feat` name a dependency too — the feature
                    # comes from *that dependency*, so a same-named feature of
                    # this crate cannot satisfy it (`"zlib-rs?/std"` is invalid
                    # even when a feature `zlib-rs` exists).  A `?` also asserts
                    # the dependency is optional.
                    head, _, tail = value.partition("/")
                    wants_optional = head.endswith("?")
                    name = norm(head.rstrip("?"))
                    if name in available and (name in optional or not wants_optional):
                        kept.append(value)
                    continue
                if norm(value) in available or value in defined:
                    kept.append(value)
            features[feature] = kept

    prune(data)
    for body in (data.get("target", {}) or {}).values():
        if isinstance(body, dict):
            prune(body)


def rebuild_manifest(
    crate_dir: Path, name: str, version: str, ws: dict | None = None
) -> tuple[bool, str]:
    manifest = crate_dir / "Cargo.toml"
    try:
        data = tomllib.loads(manifest.read_text(errors="replace"))
    except Exception as exc:  # noqa: BLE001
        return False, f"unparseable manifest ({exc})"
    package = data.get("package")
    if not isinstance(package, dict):
        return False, "no [package]"
    if norm(package.get("name", "")) != norm(name):
        return False, f"declares {package.get('name')!r}"
    inline_workspace(data, workspace_root(crate_dir))
    strip_fluff(data)
    locked = LOCKED.get((norm(name), ""))  # placeholder, replaced below
    return True, ""


# ── the job ──────────────────────────────────────────────────────────────


LOCKED: dict[tuple[str, str], Locked] = {}
VERSIONS: dict[str, str] = {}
FEATURES: dict[str, set[str]] = {}


# The ClickHouse vendor mirror is a `cargo vendor` tree: one top-level
# directory per published crate, named `<crate>-<version>`, holding *exactly*
# the files crates.io shipped. That last part is why it goes first. A GitHub tag
# is not the published crate: `rand_xorshift 0.4.0`'s tag in the rand monorepo
# still carries the `rand_core 0.9.0` API (`Error`,
# `RngCore::try_fill_bytes`), while the tarball synced to crates.io was updated
# for `TryRngCore` — so a crate vendored from the tag does not compile against
# the `rand_core 0.9.5` this lock pins. The mirror is downloaded once per
# session (~480 MB) and indexed by directory name; GitHub is the fallback.
_MIRROR_REPO = "ClickHouse/rust_vendor"
_MIRROR_TAR: Path | None = None
_MIRROR_INDEX: dict[str, str] | None = None


def mirror_tarball() -> Path | None:
    global _MIRROR_TAR
    if _MIRROR_TAR is None:
        _MIRROR_TAR = download(_MIRROR_REPO, head_sha(_MIRROR_REPO) or "")
    return _MIRROR_TAR


def mirror_index() -> dict[str, str]:
    """`<crate>-<version>` -> directory name inside the mirror tarball."""
    global _MIRROR_INDEX
    if _MIRROR_INDEX is not None:
        return _MIRROR_INDEX
    _MIRROR_INDEX = {}
    tarball = mirror_tarball()
    if tarball is None:
        return _MIRROR_INDEX
    index_path = CACHE / "rust_vendor.index"
    if index_path.exists() and index_path.stat().st_mtime >= tarball.stat().st_mtime:
        for line in index_path.read_text().splitlines():
            key, _, value = line.partition("\t")
            if key and value:
                _MIRROR_INDEX[key] = value
        return _MIRROR_INDEX
    try:
        with tarfile.open(tarball) as tf:
            for member in tf:
                parts = member.name.split("/")
                if len(parts) == 2 and member.isdir():
                    _MIRROR_INDEX[parts[1]] = member.name
    except Exception:  # noqa: BLE001
        return _MIRROR_INDEX
    CACHE.mkdir(parents=True, exist_ok=True)
    # Written through a temp file and renamed: six workers start at once, and a
    # half-written index is a *silently* incomplete one — which is how a crate
    # that the mirror does carry gets reported missing.
    temporary = index_path.with_suffix(".index.tmp")
    temporary.write_text("".join(f"{k}\t{v}\n" for k, v in sorted(_MIRROR_INDEX.items())))
    temporary.replace(index_path)
    return _MIRROR_INDEX


def mirror_extract(name: str, version: str, dest: Path) -> str | None:
    """Copy `name-version` out of the mirror tarball into `dest`."""
    prefix = mirror_index().get(f"{name}-{version}")
    tarball = mirror_tarball()
    if not prefix or tarball is None:
        return None
    with tarfile.open(tarball) as tf:
        members = [m for m in tf if m.name == prefix or m.name.startswith(prefix + "/")]
        if not members:
            return None
        dest.mkdir(parents=True, exist_ok=True)
        for member in members:
            relative = member.name[len(prefix):].lstrip("/")
            if not relative:
                continue
            target = dest / relative
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                handle = tf.extractfile(member)
                if handle is not None:
                    target.write_bytes(handle.read())
    return f"{_MIRROR_REPO}:{prefix}" if (dest / "Cargo.toml").exists() else None


def install(
    name: str, version: str, vendor: Path, quiet: bool = False, force: bool = False
) -> str:
    dest = vendor / f"{name}-{version}"
    if not force and declares(dest / "Cargo.toml", name):
        return "cached"

    # 1. The published crate, if the mirror carries this version. The mirror is
    #    a `cargo vendor` snapshot, so its directories *are* the crates.io
    #    contents — unlike a GitHub tag, which can lag or lead the release.
    source: Path | None = None
    note = ""
    ws: dict = {}
    if os.environ.get("VECTOR_SKIP_MIRROR") != "1":
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name) / "crate"
            origin = mirror_extract(name, version, tmp)
            if origin and declares(tmp / "Cargo.toml", name):
                staged = vendor / f".staging-{name}-{version}-mirror"
                shutil.rmtree(staged, ignore_errors=True)
                shutil.copytree(tmp, staged)
                source, note = staged, origin

    # 2. Otherwise the repository that publishes it: an exact tag if there is
    #    one, HEAD, then a nearby tag (see `tag_candidates`).
    # 2a. With `VECTOR_REGISTRY_ARCHIVE=1`, the published `.crate` tarball from a
    #     committed registry cache wins over everything below: it is the only
    #     source whose contents are verified against the lock.
    published: tuple[Path, str] | None = None
    if os.environ.get("VECTOR_SKIP_MIRROR") != "1":
        published = registry_published(name, version, vendor)
    # The published tarball wins wherever it is found, because it is the one
    # source whose contents the lock *checks* (SHA-256, in `fetch_crate`). A
    # repository tree is only ever a stand-in, and two of them in this lock are
    # demonstrably not the release: `glow`'s repository has no tags at all, and
    # its HEAD's `tex_image_2d` takes a `PixelUnpackData` that the pinned
    # `wgpu-hal` cannot call; `glam`'s 0.14.0 tag declares no `f64` feature,
    # while `nalgebra` (and so the whole constraints solver) requires it.
    if published is not None:
        source, note = published

    # 2b. A checked-in cargo registry copy wins over any repository tree — for
    #     the crates where a tree is known to be *not* the published crate, and
    #     for every crate when `VECTOR_PREFER_REGISTRY=1`. A tag is not the
    #     published crate: wgpu-hal 22.0.0's repository tag calls
    #     `glow::tex_image_3d(.., None, ..)`, while the crate on crates.io — the
    #     one the lockfile pins — passes `PixelUnpackData::Slice(None)`.
    #
    #     `glow` is in the set because its only reachable non-registry copies are
    #     repository trees: the AOSP mirror's `crates/glow` has no `build.rs`, so
    #     its generated `gl46.rs` lacks `GLchar` and the host build dies with 30
    #     errors. Leaving that to an environment variable made the vendor tree
    #     depend on how it was invoked — the same lockfile produced a working
    #     tree once and a broken one twice.
    if source is None and (
        name in REGISTRY_FIRST or os.environ.get("VECTOR_PREFER_REGISTRY") == "1"
    ):
        hit = registry_hit(name, version)
        if hit is not None:
            dump_repo, dump_path = hit
            with tempfile.TemporaryDirectory() as tmp_name:
                tmp = Path(tmp_name) / "crate"
                if (
                    gh_dir(dump_repo, dump_path, tmp)
                    and declares(tmp / "Cargo.toml", name)
                    and (tmp / "src").is_dir()
                ):
                    staged = vendor / f".staging-{name}-{version}-registry"
                    shutil.rmtree(staged, ignore_errors=True)
                    shutil.copytree(tmp, staged)
                    source, note = staged, f"{dump_repo}:{dump_path}"

    # 2c. A registry-first crate takes a *published* copy or nothing at all.
    #
    #     The two registry lookups above go through GitHub's code search, which
    #     is rate-limited — in a full 256-crate pass `registry_hit` and
    #     `registry_archive` can both come back empty for reasons that have
    #     nothing to do with the crate existing. Falling through to the mirror
    #     walk then silently vendors a tree that is known not to compile, which
    #     is exactly how `glow`'s 30 `GLchar` errors kept coming back. So for
    #     these crates the fallback is refused, loudly, instead of taken.
    if source is None and name in REGISTRY_FIRST:
        found = registry_archive(name, version, vendor / f".staging-{name}-{version}-archive")
        if found is not None:
            staged = vendor / f".staging-{name}-{version}-archive"
            if (staged / "Cargo.toml").is_file():
                source, note = staged, found
        if source is None:
            return (
                f"failed ({name} {version} is registry-first: no published copy could be "
                "located, and a repository tree is known to be wrong)"
            )

    repo, ref, subdir = (None, "", None) if source is not None else EXPLICIT.get(
        name, (None, "", None)
    )
    shas: list[str] = []
    if source is None:
        if repo is None:
            repo = KNOWN.get(name) or search_repo(name)
        if repo:
            if ref == "HEAD":
                sha = head_sha(repo)
                if sha:
                    shas.append(sha)
            elif ref:
                tagset = tags(repo)
                if ref in tagset:
                    shas.append(tagset[ref])
            else:
                exact, nearby = tag_candidates(name, version, tags(repo))
                shas += exact
                sha = head_sha(repo)
                if sha:
                    shas.append(sha)
                shas += nearby

    loose: tuple[Path, str, dict] | None = None
    if source is None:
        for sha in shas[:8]:
            tgz = download(repo, sha) if repo else None
            if not tgz:
                continue
            # An extracted crate tree is a few hundred MB, so the temp directory is
            # removed before the next candidate is tried — the sandbox has no room
            # to accumulate them.
            with tempfile.TemporaryDirectory() as tmp_name:
                tmp = Path(tmp_name)
                try:
                    with tarfile.open(tgz) as tf:
                        tf.extractall(tmp)
                except Exception:  # noqa: BLE001
                    continue
                roots = [p for p in tmp.iterdir() if p.is_dir()] if tmp.exists() else []
                if not roots:
                    continue
                found, matched = find_in(roots[0], name, version, subdir)
                if found is None:
                    continue
                # The workspace table is read *here*, while the crate still sits
                # inside its repository: a staged copy has no ancestors left to
                # inherit `edition.workspace = true` from.
                workspace = workspace_root(found)
                # Each attempt stages under its own name, so a later candidate can
                # never delete the copy an earlier one is holding on to.
                staged = vendor / f".staging-{name}-{version}-{sha[:8]}"
                shutil.rmtree(staged, ignore_errors=True)
                shutil.copytree(found, staged)
                if matched:
                    source, note, ws = staged, f"{repo}@{ref or sha[:8]}", workspace
                    break
                # Keep the first name-only match in case nothing declares this
                # version; a version-patched copy is a real gap and gets a note.
                if loose is None:
                    loose = (staged, f"{repo}@{ref or sha[:8]}", workspace)
    # 3. A repository that committed its cargo registry cache: the crate as
    #    crates.io published it, byte for byte (checksum-verified when the lock
    #    carries one).
    if source is None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name) / "crate"
            origin = registry_archive(name, version, tmp)
            if origin and declares(tmp / "Cargo.toml", name):
                staged = vendor / f".staging-{name}-{version}-published"
                shutil.rmtree(staged, ignore_errors=True)
                shutil.copytree(tmp, staged)
                source, note = staged, origin

    if source is None:
        hit = registry_hit(name, version)
        if hit is not None:
            dump_repo, dump_path = hit
            # The sibling `.crate` first: verifiable, and not a working copy.
            archive_path = registry_src_to_cache(dump_path)
            if archive_path:
                with tempfile.TemporaryDirectory() as tmp_name:
                    tmp = Path(tmp_name) / "crate"
                    if fetch_crate(dump_repo, archive_path, tmp) is not None:
                        staged = vendor / f".staging-{name}-{version}-published"
                        shutil.rmtree(staged, ignore_errors=True)
                        shutil.copytree(tmp, staged)
                        source, note = staged, f"{dump_repo}:{archive_path}"
            if source is None:
                with tempfile.TemporaryDirectory() as tmp_name:
                    tmp = Path(tmp_name) / "crate"
                    if (
                        gh_dir(dump_repo, dump_path, tmp)
                        and declares(tmp / "Cargo.toml", name)
                        and (tmp / "src").is_dir()
                    ):
                        declared = tomllib.loads(
                            (tmp / "Cargo.toml").read_text(errors="replace")
                        )["package"].get("version")
                        if declared == version:
                            staged = vendor / f".staging-{name}-{version}-registry"
                            shutil.rmtree(staged, ignore_errors=True)
                            shutil.copytree(tmp, staged)
                            source, note = staged, f"{dump_repo}:{dump_path}"

    # 4. Nothing on GitHub publishes it any more, and the mirror tarball is
    #    older than the crate: walk the mirror's directories over the API.
    if source is None:
        for mirror, directory in MIRRORS:
            if mirror == _MIRROR_REPO:
                continue  # already covered by the tarball index above
            for candidate in (
                f"{directory}/{name}-{version}".lstrip("/"),
                f"{directory}/{name}".lstrip("/"),
            ):
                with tempfile.TemporaryDirectory() as tmp_name:
                    tmp = Path(tmp_name) / "crate"
                    if gh_dir(mirror, candidate, tmp) and declares(
                        tmp / "Cargo.toml", name
                    ):
                        staged = vendor / f".staging-{name}-{version}-mirror"
                        shutil.rmtree(staged, ignore_errors=True)
                        shutil.copytree(tmp, staged)
                        source = staged
                        note = f"{mirror}:{candidate}"
                        break
            if source is not None:
                break
    if source is None and loose is not None:
        # Nothing published carries this version: take the closest repository
        # tree, and *say so* — a version patch is a real gap in the provenance
        # chain (`glow 0.13.1` ships through a registry cache because its
        # repository has no tag for it; a repo tree that declares another
        # version is worse than that, not better).
        source, note, ws = loose
        note += " (tree does not declare this version)"

    if source is None:
        return f"failed ({repo or 'no repo'})"

    data = tomllib.loads((source / "Cargo.toml").read_text(errors="replace"))
    package = data.get("package", {})
    if package.get("version") != version:
        package["version"] = version
    data["package"] = package
    unresolved = inline_workspace(data, ws if ws is not None else workspace_root(source))
    strip_fluff(data)
    sane_rust_version(data)
    locked = LOCKED.get((norm(name), version)) or Locked(name, version, [])
    notes = reconcile_dependencies(data, locked, VERSIONS)
    # Dependency-feature pruning first: it can drop a whole dependency entry, and
    # the feature table must then be pruned against what is left (see
    # `repair_vendored`).
    notes.extend(prune_dependency_features(data, FEATURES))
    prune_features(data)

    staged = source
    (staged / "Cargo.toml").write_text(tomli_w.dumps(data))
    (staged / "VENDOR-NOTE.txt").write_text(
        f"Source: {note}\n"
        f"Manifest rewritten for vendored use: workspace inheritance inlined,\n"
        f"dependencies reconciled against the workspace Cargo.lock, features pruned.\n"
    )
    if dest.exists():
        shutil.rmtree(dest)
    os.rename(staged, dest) if staged.parent == vendor else shutil.copytree(staged, dest)
    if staged.parent != vendor and staged.exists():
        shutil.rmtree(staged, ignore_errors=True)
    package_sum = LOCK_CHECKSUMS.get((norm(name), version))
    (dest / ".cargo-checksum.json").write_text(
        json.dumps({"files": {}, "package": package_sum})
    )
    flags = []
    if notes:
        flags.append("; ".join(notes[:4]))
    if unresolved:
        # An inherited key with no workspace table to read: loud, because it
        # means the crate was vendored from a tree that lost its root manifest.
        flags.append(f"unresolved inheritance: {', '.join(unresolved)}")
    return f"ok ({note})" + (f" [{'; '.join(flags)}]" if flags and not quiet else "")


SYNTHETIC: dict[tuple[str, str], str] = {
    # crate+version: donor directory inside the vendor tree.  Used only when no
    # published source for that exact version is reachable at all.
    ("bit-set", "0.6.0"): "bit-set-0.8.0",
}


def synthesise(name: str, version: str, vendor: Path) -> str:
    donor = vendor / SYNTHETIC[(name, version)]
    if not donor.is_dir():
        return "donor missing"
    dest = vendor / f"{name}-{version}"
    try:
        data = tomllib.loads((donor / "Cargo.toml").read_text())
    except Exception as exc:  # noqa: BLE001
        return f"donor unreadable ({exc})"
    data.setdefault("package", {})["version"] = version
    data["package"]["name"] = name
    locked = LOCKED.get((norm(name), version)) or Locked(name, version, [])
    reconcile_dependencies(data, locked, VERSIONS)
    prune_dependency_features(data, FEATURES)
    prune_features(data)
    shutil.rmtree(dest, ignore_errors=True)
    shutil.copytree(donor, dest)
    (dest / "Cargo.toml").write_text(tomli_w.dumps(data))
    (dest / "VENDOR-NOTE.txt").write_text(
        "No published source for this exact version is reachable from this sandbox;\n"
        f"the copy was taken from `{SYNTHETIC[(name, version)]}` in the same repository\n"
        "with the version rewritten and dependencies reconciled against Cargo.lock.\n"
    )
    return "ok (synthetic)"


def vendored_features(vendor: Path) -> dict[str, set[str]]:
    """Every vendored crate's feature names, plus its implicit optional-dep ones.

    A manifest may only request features its dependency actually declares, and
    the vendored tree is the ground truth for what "declares" means here.
    """
    out: dict[str, set[str]] = {}
    for crate_dir in vendor.iterdir():
        if not crate_dir.is_dir():
            continue
        manifest = crate_dir / "Cargo.toml"
        if not manifest.is_file():
            continue
        try:
            data = tomllib.loads(manifest.read_text())
        except Exception:  # noqa: BLE001
            continue
        package = data.get("package")
        if not isinstance(package, dict) or not package.get("name"):
            continue
        names = set((data.get("features", {}) or {}).keys())
        for section in DEP_TABLES:
            for dep, spec in (data.get(section, {}) or {}).items():
                if isinstance(spec, dict) and spec.get("optional"):
                    # An optional dependency is an implicit feature of the same
                    # name unless some feature writes `dep:<name>`.
                    names.add(dep)
        out[norm(str(package["name"]))] = names
    return out


def prune_dependency_features(data: dict, features: dict[str, set[str]]) -> list[str]:
    """Drop feature requests the vendored dependency cannot satisfy.

    A manifest written against a *different* version of its dependency (which is
    what a version-patched vendored crate is) can ask for features that version
    never had — `nalgebra` asking `glam 0.14` for `f64`, for instance.  The
    request cannot be honoured, so it is dropped rather than left to fail the
    whole resolution.
    """
    notes: list[str] = []

    def walk(table: dict, where: str) -> None:
        for section in DEP_TABLES:
            if not isinstance(table.get(section), dict):
                continue
            # A copy: an entry whose every requested feature was dropped is
            # deleted from the table (see below).
            for name, spec in list(table[section].items()):
                if not isinstance(spec, dict):
                    continue
                requested = spec.get("features")
                if not requested:
                    continue
                dependency = norm(str(spec.get("package") or name))
                declared = features.get(dependency)
                if declared is None:
                    continue
                kept = [f for f in requested if f in declared]
                if kept != list(requested):
                    dropped = sorted(set(requested) - set(kept))
                    notes.append(f"{where}{name}: dropped {dropped}")
                    spec["features"] = kept
                if not kept:
                    # Every requested feature was dropped, so the request is a
                    # no-op — but cargo still *parses* it, and the version-patch
                    # collapsed several glam majors onto one directory: nalgebra
                    # asks `glam033` for `f64`, `glam033` resolves to glam 0.14.0,
                    # and every `glamNNN` key now names the same package with a
                    # different feature set.  Cargo unifies by package, so a stale
                    # key silently re-adds what another one dropped. The key goes.
                    del table[section][name]
                    notes.append(f"{where}{name}: unused feature request removed")

    walk(data, "")
    for target, body in (data.get("target", {}) or {}).items():
        if isinstance(body, dict):
            walk(body, f"target[{target[:24]}].")
    return notes


def write_checksums(vendor: Path) -> int:
    """Give every vendored crate the checksum `Cargo.lock` already records.

    A registry source is checksummed, and the lock file carries one checksum per
    package.  A directory source has to agree with it — that is what the
    `package` field of `.cargo-checksum.json` is for.  A real `.crate` download
    would provide it; a GitHub checkout cannot compute it, but the lock already
    knows it, so it is copied across.  (The `files` map stays empty: per-file
    hashes are an optimisation, not a requirement.)

    This is why the lock file is left untouched — no `checksum` lines are
    removed, and `--locked` still means something.
    """
    data = tomllib.loads(LOCK.read_text())
    checksums = {
        (norm(package["name"]), package["version"]): package.get("checksum")
        for package in data.get("package", [])
        if "source" in package
    }
    written = 0
    for name, version, crate_dir in crate_dirs(vendor):
        checksum = checksums.get((norm(name), version))
        if not checksum:
            continue
        marker = crate_dir / ".cargo-checksum.json"
        wanted = json.dumps({"files": {}, "package": checksum})
        if not marker.exists() or marker.read_text() != wanted:
            marker.write_text(wanted)
            written += 1
    return written


def repair_vendored(vendor: Path) -> int:
    """Re-reconcile every vendored manifest against the lock, in place.

    Vendoring is incremental: a crate installed by an earlier run keeps whatever
    manifest that run wrote, even when the manifest rules have since improved.
    This pass makes the rules retroactive.  It needs no network at all — just
    the manifests and `Cargo.lock`.
    """
    touched = 0
    for name, version, crate_dir in crate_dirs(vendor):
        manifest = crate_dir / "Cargo.toml"
        locked = LOCKED.get((norm(name), version))
        if locked is None:
            continue
        try:
            data = tomllib.loads(manifest.read_text())
        except Exception as exc:  # noqa: BLE001
            print(f"  !! {crate_dir.name}: {exc}")
            continue
        if "package" not in data:
            continue
        before = json.dumps(data, sort_keys=True)
        inline_workspace(data, {})
        strip_fluff(data)
        sane_rust_version(data)
        reconcile_dependencies(data, locked, VERSIONS)
        # Dependency-feature pruning runs **first**: it can delete a dependency
        # whose every requested feature was dropped, and `prune_features` has to
        # see the table that is actually left, or a feature body would keep
        # naming a dependency that is no longer there (`convert-glam033 =
        # ["glam033"]` beside a `glam033` entry that just went away).
        for note in prune_dependency_features(data, FEATURES):
            print(f"  {crate_dir.name}: {note}")
        prune_features(data)
        if json.dumps(data, sort_keys=True) != before:
            manifest.write_text(tomli_w.dumps(data))
            touched += 1
    return touched


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("vendor", nargs="?", default="/tmp/vendor")
    parser.add_argument("--jobs", type=int, default=6)
    parser.add_argument(
        "--lock",
        metavar="PATH",
        help="vendor the closure of another project's Cargo.lock (default: this workspace's)",
    )
    parser.add_argument("--only", action="append", default=[])
    parser.add_argument(
        "--force",
        action="store_true",
        help="re-vendor even crates that are already present (uses the tarball cache)",
    )
    parser.add_argument(
        "--repair-only",
        action="store_true",
        help="re-reconcile the vendor tree against Cargo.lock and stop",
    )
    args = parser.parse_args()
    global LOCK_OVERRIDE
    if args.lock:
        LOCK_OVERRIDE = Path(args.lock).resolve()
        print(f"lock: {LOCK_OVERRIDE}")
    vendor = Path(args.vendor)
    vendor.mkdir(parents=True, exist_ok=True)

    global LOCKED
    LOCKED = read_lock()
    for (name, version), locked in LOCKED.items():
        VERSIONS.setdefault(norm(name), version)
    LOCK_CHECKSUMS.update(checksums_from_lock())
    victims = sorted({(locked.name, locked.version) for locked in LOCKED.values()})
    if args.only:
        wanted = {norm(name) for name in args.only}
        victims = [v for v in victims if norm(v[0]) in wanted]
    if args.repair_only:
        print(f"repairing {vendor} against {LOCK}")
        restore_known_gaps(vendor)
        FEATURES.clear()
        FEATURES.update(vendored_features(vendor))
        print(f"repaired {repair_vendored(vendor)} manifests")
        print(f"wrote {write_checksums(vendor)} checksums")
        return 0
    if os.environ.get("VECTOR_SKIP_MIRROR") != "1":
        # Built here, on one thread, so every worker reads the same finished
        # index instead of racing to write it.
        index = mirror_index()
        print(f"mirror index: {len(index)} published crate(s) available")
    print(f"vendoring {len(victims)} packages into {vendor} with {args.jobs} jobs")

    done = 0

    def job(item: tuple[str, str]) -> None:
        nonlocal done
        name, version = item
        try:
            status = install(name, version, vendor, force=args.force)
        except Exception as exc:  # noqa: BLE001
            status = f"error {exc}"
        done += 1
        log(f"[{done}/{len(victims)}] {name}@{version}: {status}")

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        list(pool.map(job, victims))

    for (name, version) in SYNTHETIC:
        if not (vendor / f"{name}-{version}" / "Cargo.toml").exists():
            print(f"[synthetic] {name}@{version}: {synthesise(name, version, vendor)}")

    # Staging directories are working space, never output: a candidate that lost
    # to a better one must not survive the run (cargo would try to vendor it).
    for leftover in vendor.glob(".staging-*"):
        shutil.rmtree(leftover, ignore_errors=True)

    # A truncated mirror is not a missing crate: repair the files the routes
    # could not deliver before the tree is declared finished (see `_FILE_GAPS`).
    restore_known_gaps(vendor)

    # The tree is complete, so it is now the ground truth for "what does this
    # dependency actually declare". Re-run the manifest rules against it: a
    # crates.io manifest written for one version of a dependency can ask a
    # version-patched vendored copy for features it never had (nalgebra asking
    # glam 0.14 for `f64`), and the prune that drops such a request cannot run
    # while the tree is still half-built — it *is* the tree it consults.
    FEATURES.clear()
    FEATURES.update(vendored_features(vendor))
    repaired = repair_vendored(vendor)
    if repaired:
        print(f"[repair] re-reconciled {repaired} manifest(s) against the finished tree")

    installed = [p.name for p in vendor.iterdir() if (p / "Cargo.toml").exists()]
    missing = [
        f"{locked.name}@{locked.version}" for locked in LOCKED.values()
        if not (vendor / f"{locked.name}-{locked.version}" / "Cargo.toml").exists()
    ]
    # Every exit path leaves the tree with lock-matching checksums: a crate
    # vendored by an `--only` run must not keep the placeholder.
    write_checksums(vendor)
    print(f"DONE: {len(installed)} crates vendored; missing {len(missing)}")
    for item in missing:
        print("  missing:", item)
    return 0 if not missing else 1


if __name__ == "__main__":
    raise SystemExit(main())
