#!/usr/bin/env bash
# Fails when the generated API types (web/src/api/gen, android/…/dev/shed/api)
# are not what `cargo run -p shed-codegen` makes from crates/shed-api now.
# For CI, and before committing a change to shed-api.
set -euo pipefail
cd "$(dirname "$0")/.."
generated=(web/src/api/gen android/app/src/main/java/dev/shed/api)
cargo run --quiet -p shed-codegen
if ! git diff --exit-code -- "${generated[@]}" \
    || [ -n "$(git ls-files --others --exclude-standard -- "${generated[@]}")" ]; then
  echo "The API types were stale: commit what cargo run -p shed-codegen wrote." >&2
  exit 1
fi
