#!/bin/bash
#
# Compares pure RBS against librbs by timing the real `rbs list` command
# on the kaigionrails/conference-app workload: run from inside the clone
# (see below), it loads the app's own `sig/` via `-I` plus the gem
# collection auto-discovered from `rbs_collection.yaml`.
#
# `rbs list` internally runs exactly the pipeline we want to measure:
#
#     Environment.from_loader(loader).resolve_type_names
#
# then iterates `class_decls` / `class_alias_decls` / `interface_decls`
# (RBS::CLI#run_list). Under librbs that iteration triggers the one-shot
# `materialize_all`, so a single `rbs list` invocation covers
# load + resolve (+ materialize for librbs) — and nothing else: no type
# check, no validate.
#
# The command's wall time also includes a fixed startup cost (VM boot +
# `require "rbs"`). We measure that separately with `rbs version` so the
# load+resolve+materialize component can be reported on its own.
#
# Usage:
#     RBENV_VERSION=4.0.4 benchmark/list_benchmark.sh
#     RUNS=30 RBENV_VERSION=4.0.4 benchmark/list_benchmark.sh
#
# Prerequisites (one-shot): build the extension for the target Ruby, then
# clone conference-app (pinned) and install its gem collection cache —
#     RBENV_VERSION=4.0.4 bundle install && RBENV_VERSION=4.0.4 bundle exec rake compile
#     cd benchmark/fixtures
#     git clone --filter=blob:none --sparse https://github.com/kaigionrails/conference-app.git
#     cd conference-app
#     git sparse-checkout set --no-cone /sig /rbs_collection.yaml /rbs_collection.lock.yaml /Gemfile /Gemfile.lock
#     BUNDLE_GEMFILE="$PWD/Gemfile" bundle install   # installs the app's type: rubygems gems
#     rbs collection install --frozen
# The clone is intentionally NOT pinned — it tracks the default branch so
# runs use a current app; record the measured commit with your results.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# conference-app is cloned (not vendored) into fixtures/ by the setup
# below; nothing from it is committed to this repo.
APP="$ROOT/benchmark/fixtures/conference-app"
SIG="$APP/sig"
LIB="$ROOT/lib"
DRIVER="$ROOT/benchmark/list_driver.rb"

RUBY="${RUBY:-$(rbenv which ruby 2>/dev/null || command -v ruby)}"
export RUNS="${RUNS:-20}"
export WARMUP="${WARMUP:-3}"

# Strip bundler env so the child sees the full local gemset: the rbs
# default gem plus the conference-app gems whose `type: rubygems` sigs
# ship inside the gems themselves (installed via the clone's Gemfile).
strip() { env -u BUNDLE_GEMFILE -u BUNDLE_BIN_PATH -u RUBYOPT -u BUNDLER_VERSION -u GEM_HOME -u GEM_PATH "$@"; }

RBS_EXE="$(strip "$RUBY" -e 'print Gem.bin_path("rbs", "rbs")')"

[ -d "$SIG" ] && [ -d "$APP/.gem_rbs_collection" ] || {
  echo "conference-app clone / collection cache missing under $APP -- see prerequisites in this script" >&2
  exit 1
}

echo "## rbs -Isig list  ($("$RUBY" -v), RUNS=$RUNS)"
echo

# Run from inside the clone: `rbs` auto-discovers `rbs_collection.yaml`
# (the standard filename), so no --collection flag is needed — the same
# `rbs -I sig list` a conference-app developer would type in their app.
cd "$APP"

# End-to-end `rbs list`: load + resolve (+ materialize for librbs) + print.
strip "$RUBY" "$DRIVER" "pure/list"   "$RUBY"                    "$RBS_EXE" -I sig list
strip "$RUBY" "$DRIVER" "librbs/list" "$RUBY" -I "$LIB" -r librbs "$RBS_EXE" -I sig list

# Startup baseline: VM boot + require rbs (+librbs), no environment work.
strip "$RUBY" "$DRIVER" "pure/boot"   "$RUBY"                    "$RBS_EXE" version
strip "$RUBY" "$DRIVER" "librbs/boot" "$RUBY" -I "$LIB" -r librbs "$RBS_EXE" version
