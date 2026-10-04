# Elasticsearch CAT views

These views make Elasticsearch CAT output easier to scan by highlighting useful
metrics, sorting related rows, and hiding less relevant columns. They help you
explore cluster health, resource usage, and workload distribution.

## Using the views

Copy `examples/views/elasticsearch/` into `~/.config/tview/views/`:

```sh
mkdir -p ~/.config/tview/views
cp -R examples/views/elasticsearch ~/.config/tview/views/
```

If you set `XDG_CONFIG_HOME`, use `$XDG_CONFIG_HOME/tview/views/` instead.
Directory symlinks are not scanned.

Views load automatically for matching `cat_*.txt` filenames:

```sh
tview /path/to/cat_nodes.txt
```

Use `--view cat_nodes` to select a view explicitly, or `--no-view` to open the
file without a view. Press `v` to inspect or edit the view configuration. Hidden
columns remain available in the input.

## Available views

| View | What it highlights |
| --- | --- |
| `cat_aliases` | Alias-to-index mappings and write targets. |
| `cat_allocation` | Disk usage and shard distribution across nodes. |
| `cat_fielddata` | The largest fielddata memory consumers. |
| `cat_health` | Cluster health, unassigned primaries, and pending work. |
| `cat_indices` | Index storage usage and deleted-document counts. |
| `cat_nodeattrs` | Node attributes, including awareness and tier groupings. |
| `cat_nodes` | Heap, CPU, and disk usage alongside indexing and search activity. |
| `cat_pending_tasks` | The oldest queued cluster-state tasks and their priorities. |
| `cat_recovery` | Recovery stages, elapsed time, and progress. |
| `cat_repositories` | Registered snapshot repositories and their types. |
| `cat_segments` | Segment document counts, deleted documents, and memory sizes. |
| `cat_shards` | Shard states, unassigned shards, and storage sizes. |
| `cat_templates` | Template names and the first token of each index pattern. |
| `cat_thread_pool` | Thread-pool queues, active work, and rejection counts. |

## Reading the colors

- **Health colors** follow Elasticsearch's red, yellow, and green status meanings.
  Green health does not guarantee good performance.
- **Disk, heap, and CPU colors** draw attention to higher usage. Treat them as
  starting points for investigation, not alerts: cluster settings, node roles,
  hardware, and workload all matter. Nearly full caches can be normal on
  dedicated frozen nodes.
- **Relative gradients** highlight larger values within the loaded file, not
  fixed capacity limits. Recovery colors indicate progress.
- **Identifier colors** help group matching values; they do not indicate severity.

## Keep in mind

CAT files are snapshots. Compare captures to understand trends; thread-pool
rejections are cumulative counts, not current rejection rates. Repository listings
show registration, not backup health.

Views depend on the column names in the file, so different Elasticsearch versions
or capture formats may need configuration adjustments. Missing fields do not
mean zero usage. Sizes and durations sort numerically, but mixed byte units can
have small ordering differences; use normalized bytes for exact comparisons.

The templates view is a limited inventory, not a full template audit: whitespace
parsing cannot reliably preserve patterns containing spaces or all metadata.
Use structured output for complete template details. A header-only pending-tasks
file may appear as a single header row; it represents no captured tasks.
