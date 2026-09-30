# Spark

A terminal task manager written in Rust. It shows live processes, supports sorting/filtering, and includes a Docker view with container stats enabling logs and exec for any docker container.

## Build (Linux)

```bash
cargo build --release
```

Binary output:

```
target/release/spark
```

## Run

```bash
cargo run
```

## Install (Linux)
Installer made for Ubuntu.

```bash
./install.sh
```

By default the install script copies the binary to `~/.local/bin/spark`.
You can override the destination prefix:

```bash
PREFIX=/opt/spark ./install.sh
```

The install script also creates a desktop entry at
`~/.local/share/applications/spark.desktop` so the app appears in the
Ubuntu launcher as "Spark". On Ubuntu it uses `gnome-terminal` with a custom
WM_CLASS so it can be pinned separately in the dash. The installer detects
Wayland via `GDK_BACKEND`, `WAYLAND_DISPLAY`, or `XDG_SESSION_TYPE` and forces
`GDK_BACKEND=x11` for the launcher entry. If you want to override this, re-run
the installer with:

```bash
FORCE_X11=1 ./install.sh
```

## Install from Git (Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/arch-ai/spark/main/install-from-git.sh | bash
```

## Notes

- Docker view requires the `docker` CLI in `PATH`.
- Container shell uses `docker exec` and opens a new terminal window.

## Process memory

The process table shows **RAM** for the individual process and **TREE** for that
process plus its descendants. Memory sorting uses TREE, so an application's child
processes contribute to its position. `z` expands or collapses the tree. Search
keeps the full tree totals even when child names do not match. Threads share an
address space and are excluded from these sums.

On Linux, RAM uses **PSS** (proportional set size): shared memory is divided among
the processes sharing it. If PSS is unavailable, a `~` marks an estimate using
resident memory (RSS). A TREE total is marked `~` when any member uses RSS. Wider
terminals also show **SWAPtree**, the proportional swap total; `?` means at least
one member's swap usage is unknown. Display units K/M/G mean KiB/MiB/GiB.

PSS is sampled from `smaps_rollup` in the background, at most once per five seconds
per process. Each scan starts at most 256 reads and stops starting reads after
50 ms; a single kernel read may exceed that budget. Samples older than 15 seconds
fall back to RSS until refreshed. Cached samples are discarded when a process
exits or its start time changes. No elevated permissions are requested.

Expanded TREE totals overlap with their child rows. System RAM also includes
memory outside process mappings, so it need not equal the process totals. See the
[Linux memory accounting documentation](https://docs.kernel.org/filesystems/proc.html).

## Docker controls

Press `d` to switch between Docker and processes. In Docker:

| Key | Action |
| --- | --- |
| `↑` / `↓` | Select a container or Compose project |
| `/`, `x` | Filter, clear filter |
| `Enter` | Open the selected container's shell |
| `l`, `e` | View logs or environment |
| `i`, `v`, `a` | Browse images, volumes, or containers with sizes |
| `F5` | Refresh Docker, or retry an open resource list |
| Right click | Container/project actions and disk cleanup menus |
| `Esc` | Close a modal, including a loading logs/inspect window |
| `q` / `Ctrl+C` | Quit (`Ctrl+C` also works in modals) |

Container selection follows its ID across refreshes. Short terminals prioritize the
container table; resource lists remain available with `i` / `v` / `a`.
The table distinguishes loading, empty, filtered, unavailable, and stale data.
Host ports exclude ports that are only exposed inside a container.

Docker reads run in the background with a 15-second limit per command; lifecycle
and most cleanup commands allow 120 seconds. Volume deletion and pruning allow
30 minutes for large data sets. A timeout stops the local CLI process;
an operation already accepted by Docker may still complete. Refresh and check its
state before retrying. Output is limited to 8 MiB per stream.

### Volume details

Press `v`, select a volume, then press `Enter` or `i` to open its details.
The volume list shows size and **container activity**; `F5` refreshes both.
Selecting a volume shows its attached **Containers** and their **Images** below
the table. Details show the full names and each container's image and Compose
project separately. Unattached volumes show `None`; unavailable data shows `Unknown`.
Details include disk usage, reference count, mountpoint, creation time, attached
containers and the most recent known container stop, with its timestamp and age.

Docker does not record a volume's last file-access time. Activity uses existing
containers as evidence: `Attached now`, `Paused`, `Restarting`, or `Stop 2d ago`.
Orphan volumes and missing history show `Unknown`; creation time is never used as
last-use time. Removed containers' usage history cannot be reconstructed this way.
Drivers that cannot report size, and failed queries, also show `Unknown` with an
explanation in details. See the [Docker volume API fields](https://docs.docker.com/reference/api/engine/version/v1.51/#tag/Volume).

To delete a selected volume, press `Delete` or right-click and choose **Delete
Volume**, then confirm. Deletion runs in the background with an elapsed-time
indicator; you can keep browsing. Duplicate deletions are blocked until the current
operation finishes. Success and full errors stay in a scrollable result window
until dismissed. If another dialog is open, the result appears after it closes.

Deleting a volume also removes **every container attached to it**, including running
containers, before removing the volume. The confirmation warns about both removals.
Spark matches exact volume mounts and removes containers by ID; their other volumes
are retained. If removing a container fails, volume deletion stops and the result
lists any containers already removed. A newly attached container can still prevent
volume removal; refresh details before retrying. See
[Docker volume rm](https://docs.docker.com/reference/cli/docker/volume/rm/).

## Validation

```bash
cargo test --offline
cargo clippy --offline --all-targets
cargo build --offline
python3 scripts/qa_docker_tui.py target/debug/spark
```

The terminal smoke test uses a temporary fake Docker CLI to exercise slow requests,
daemon errors, navigation, and terminal restoration without changing real containers.
It also checks volume details, long-name menu clicks, slow deletion, duplicate
prevention, persistent errors, and successful removal using fake volumes.
Set `SPARK_SNAPSHOT_DIR=/tmp/spark-ui` when running the tests to export Docker
render snapshots at 140×36, 100×24, 60×20, and 40×16.
