#!/bin/bash
# Run a cargo command against the vendored dependency tree.
#   tools/check.sh [cargo args…]     e.g. tools/check.sh check --workspace
set -o pipefail
export PATH=/tmp/rust/prefix/bin:$PATH
export CARGO_HOME=/tmp/cargohome
export PYTHONPATH=/tmp/pylibs
mkdir -p /tmp/cargohome
cat > /tmp/cargohome/config.toml <<'CONF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "/tmp/vendor"
CONF
cd "$(dirname "$0")/.."
if [ ! -d /tmp/vendor ] || [ "$(ls /tmp/vendor 2>/dev/null | wc -l)" -lt 10 ]; then
  echo "vendor tree missing — rebuilding" >&2
  python3 tools/vendor_deps.py >/dev/null 2>&1
fi
cmd="${1:-check}"; shift || true
exec cargo "$cmd" "$@" --offline
