#!/usr/bin/env bash

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

tview() {
    if [ -n "${TVIEW_BIN:-}" ]; then
        "$TVIEW_BIN" "$@"
    else
        cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- "$@"
    fi
}

render() {
    printf 'Command: tview --output table'
    for argument do
        printf ' %s' "$argument"
    done
    printf '\nReformatted table:\n'
    tview --output table "$@"
    printf '\n'
}

show_raw_input() {
    echo "Raw input:"
    cat "$1"
    printf '\n\n'
}

# Delimited input with tab-separated columns and Windows line endings.
echo "Delimited input: detects tab-separated columns and Windows line endings"
show_raw_input "examples/data/windows_newlines.csv"
render "examples/data/windows_newlines.csv"

# A JSON array of objects.
echo "---"
echo "JSON input: flattens an array of objects into rows and columns"
show_raw_input "examples/data/json/array-of-objects.json"
render "examples/data/json/array-of-objects.json"

# Newline-delimited JSON with a column discovered in a later record.
echo "---"
echo "NDJSON input: discovers a column introduced by a later record"
show_raw_input "examples/data/json/records.ndjson"
render --format ndjson "examples/data/json/records.ndjson"

# A table nested inside an Elasticsearch-style response.
echo "---"
echo "JSON Pointer: selects and flattens hits from an Elasticsearch response"
show_raw_input "examples/data/json/elasticsearch-response.json"
render --format json --json-path /hits/hits \
    "examples/data/json/elasticsearch-response.json"
