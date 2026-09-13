# Tview

Browse delimited text, JSON, NDJSON, SQLite databases, and Elasticsearch in your
terminal. Search, sort, filter, and hide columns, then save the view or export
the displayed data.

Tview is an independent Rust rewrite of [Tabview](https://github.com/Tabviewer/tabview).
See [screenshots](screenshots/) for the interface.

## Install

Install from crates.io with Rust 1.90.0 or later:

```sh
cargo install tview
```

SQLite and saved views are included. Add clipboard or Elasticsearch support
separately with `--features clipboard` or `--features elasticsearch`. See
[installation](docs/installation.md) for build options and
[migration](docs/migration.md) if you used Tabview.

## Get started

```sh
tview data.csv
tview records.ndjson
tview response.json --json-path /hits/hits
tview sample/us-counties.sqlite3
tview - < data.csv
```

Move with the arrow keys or `h`, `j`, `k`, `l`. Press `/` to search, `f` to
filter the current column, and `Enter` to read a full cell. Press `?` for help
and `q` to quit. The [keybinding reference](docs/keybindings.md) lists all controls.

Redirected output defaults to a text table. Choose JSON or JSONL explicitly:

```sh
tview data.csv > table.txt
tview data.csv --output json > rows.json
tview data.csv --interactive --output table > selected.txt
```

The last command opens the viewer and exports your final view when you quit.
Use a different output path from the input. JSON exports contain displayed
strings, including saved-view formatting. See the [CLI contract](docs/cli-contract.md)
for schemas, output behavior, and exit codes.

## Guides

- [File input](docs/file-input.md), [SQLite](docs/sqlite.md), and
  [Elasticsearch](docs/elasticsearch.md)
- [Saved views](docs/saved-views.md) and [color themes](docs/themes.md)
- [Large files and schema discovery](docs/large-files.md)
- [Contributing](docs/contributing.md) and [release process](docs/releases.md)

The [documentation index](docs/index.md) lists every guide.
Tview retains Tabview's [MIT license](LICENSE.txt) and
[contributor credits](docs/credits.md).
