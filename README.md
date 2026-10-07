# Spark

A Linux terminal task manager written in Rust, with process trees, listening ports,
Docker containers and storage, native Node/PM2 processes, and project workspaces.

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

## Navigation and actions

| Key | Action |
| --- | --- |
| `1`, `2`, `3`, `4`, `5` | Processes, Ports, Docker, Node/PM2, Projects |
| `↑` / `↓`, `Home` / `End`, `PageUp` / `PageDown` | Move through the active table |
| `/`, `x` | Filter, clear filter |
| `F5` | Refresh the active view or retry an open resource list |
| `F10` or right click | Open actions for the selected row |
| `F2` | Open the shared resource inspector |
| `W` | Open saved named filters |
| `↑` / `↓`, `Enter`, `Esc` in a menu | Select, run, close |
| Click a column header | Sort by that column; click again to reverse |
| `s`, `r` | Choose the active table's sort field, reverse its direction |
| `?` | Open scrollable keyboard help |
| `Space` | Pause or resume the sidebar logo animation |
| `e`, `l` | View the selected process/container's environment or logs |
| `k` | Stop the selected process or container |
| `Esc` | Close the active dialog or leave filter input |
| `q` / `Ctrl+C` | Quit (`Ctrl+C` also works in dialogs) |

Processes, Ports, Docker, Projects, and both Node tabs share a header with view details on
the left and Search on the right. Narrow terminals stack these controls.

Click a column header to sort its table. **▲** marks ascending order and **▼**
marks descending order; the active header is cyan. Clicking the same header
reverses the order. Each Node tab keeps its own sort order.
Each table saves its own sort choice across launches. The `s` menu
shows its available fields; selecting the current field reverses it. Process
sorting also uses `c` (CPU), `m` (tree memory), and `n` (name). PID sorting is
available through the menu. Missing sizes, activity, and other measurements stay
last in either direction. Numeric fields sort by their values, including sizes
with different units. Grouped rows retain their project or application grouping.
Short screens prioritize table rows and retain the important names and values.
The sidebar hides below 60 columns; numeric navigation remains available.
The ASCII logo animates at most five times per second within the existing UI loop.
Animation pauses while a dialog is open and when the logo is hidden.

Process stops, container actions, PM2 actions, and storage cleanup run in the
background. You can keep browsing while they run. Repeating an action on the same
target is blocked until it finishes. Failures stay in a scrollable result window;
results wait for an open dialog to close rather than replacing its contents.

## Process memory

The process table shows **RAM** for the individual process and **TREE** for that
process plus its descendants. The default memory sort and `m` shortcut use TREE,
so an application's child processes contribute to its position. Click **RAM** to
sort by individual memory or **TREE** to sort by the total including children.
The SWAPtree and USER headers also support sorting. `z` expands or collapses the tree. Search
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
Expired measurements take priority over newly started processes, so process churn
cannot continually push existing applications back to RSS estimates. Process
actions also check kernel start ticks before signaling, to guard against PID reuse.

Expanded TREE totals overlap with their child rows. System RAM also includes
memory outside process mappings, so it need not equal the process totals. See the
[Linux memory accounting documentation](https://docs.kernel.org/filesystems/proc.html).

## Docker controls

Press `3` to open Docker, or `d` to switch between Docker and processes. In Docker:

| Key | Action |
| --- | --- |
| `↑` / `↓` | Select a container or Compose project |
| `/`, `x` | Filter, clear filter |
| `Enter` | Open the selected container's shell |
| `l`, `e` | View logs or environment |
| `i`, `v`, `a` | Browse images, volumes, or containers with sizes |
| `s`, `r` | Sort containers or the open resource list, reverse direction |
| `F5` | Refresh Docker, or retry an open resource list |
| `F10` or right click | Container/project actions; right click also opens disk cleanup menus |
| `Esc` | Close a modal, including a loading logs/inspect window |
| `q` / `Ctrl+C` | Quit (`Ctrl+C` also works in modals) |

Container selection follows its ID across refreshes. Short terminals prioritize the
container table; resource lists remain available with `i` / `v` / `a`.
The table distinguishes loading, empty, filtered, unavailable, and stale data.
Host ports exclude ports that are only exposed inside a container.

The **RAM** column shows each running container's live memory usage. Click its
header to sort; unknown and stale measurements stay last in either direction.
`F2` → **Details** shows usage, memory limit, percentage of that limit, and sample
age. `-` means the container is not running; `?` means no valid measurement is
available. A trailing `*` marks a cached measurement after a stream failure or
15 seconds without an update. `F5` reconnects the stats stream. K/M/G are KiB/MiB/GiB.

Measurements come from one `docker stats` stream while Docker or project memory
is being viewed, without additional recurring CLI queries. On Linux these values
exclude inactive file cache, matching the Docker CLI; see
[Docker stats](https://docs.docker.com/reference/cli/docker/container/stats/).
The reported limit can be the daemon host's memory when no container limit is set.

Docker reads run in the background with a 15-second limit per command; lifecycle
and most cleanup commands allow 120 seconds. Volume deletion and pruning allow
30 minutes for large data sets. A timeout stops the local CLI process;
an operation already accepted by Docker may still complete. Refresh and check its
state before retrying. Output is limited to 8 MiB per stream.

### Volume details

Press `v`, select a volume, then press `Enter` or `i` to open its details.
The volume list shows size and **container activity**; `F5` refreshes both.
Selecting a volume shows its attached **Containers**, their **Images**, and
**Projects** with directories below the table. Details show the full names and
each container's image, Compose project, and project directory together.
Directories come from the container's Compose working-directory label; missing
labels show `Unknown`, and containers outside Compose show `Unmanaged`.
Unattached volumes show `None`; unavailable data shows `Unknown`.
Details include disk usage, reference count, mountpoint, creation time, attached
containers and the most recent known container stop, with its timestamp and age.

Press `c` in the volume list, or choose **Show Containers**, to see only the
containers attached to the selected volume. Press `x` to clear that scope.
Volume sorting supports size, name, and activity. Unattached volume names are
dimmed; attached names are bold. Sorting and `F5` refresh keep the selected volume
when it still exists.

Docker does not record a volume's last file-access time. Activity uses existing
containers as evidence: `Attached now`, `Paused`, `Restarting`, or `Stop 2d ago`.
Orphan volumes and missing history show `Unknown`; creation time is never used as
last-use time. Removed containers' usage history cannot be reconstructed this way.
Drivers that cannot report size, and failed queries, also show `Unknown` with an
explanation in details. See the [Docker volume API fields](https://docs.docker.com/reference/api/engine/version/v1.51/#tag/Volume).

To delete a selected volume, press `Delete` or open its actions and choose **Delete
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

Cleanup confirmations describe the selected scope. Volume pruning uses Docker's
default scope of unused **anonymous** volumes; deleting a named volume uses its
individual Delete action. See [Docker volume prune](https://docs.docker.com/reference/cli/docker/volume/prune/).

## Ports

The Ports view shows listening TCP and unconnected UDP sockets, including
listeners whose owners are inaccessible. Rows show protocol and external/internal
container bindings. Docker proxy listeners resolve to their container names.
Filtering includes both sides of a port binding.

A Docker query failure keeps fresh host listeners and marks retained container
bindings as cached. `F5` retries. Actions are unavailable for unknown owners.
Selecting a row preserves that listener or container across refreshes.

## Node and PM2

Native Node processes load independently of PM2, so a slow PM2 query leaves native
data and navigation available. PM2 failures retain the last successful list with
a visible error and `F5` retry. The native table uses RSS; clustered native workers
sharing a script have their CPU and RSS totals grouped together.

The Node screen opens on the **Node.js Processes** tab. Click **PM2**, or use `Tab`
or `Shift+Tab`, to switch tabs. Each tab uses the full table area and keeps its own
selection, scroll position, and sort order. Tabs remain available when a list is
empty or PM2 is unavailable; `F5` retries failed reads. PM2 actions apply to the
selected row: `Ctrl+R` restarts, `Ctrl+S` stops, and `Ctrl+T` starts; `F10` exposes
these actions plus logs, environment, and working directory. Hover does not change
the action target. PM2 reads allow 15 seconds and actions allow 120 seconds.

Hidden resource workers sleep until their view resumes or Projects/the inspector needs their snapshots. The main UI refreshes
system CPU and RAM without repeating the background workers' full process scans.

## Projects and shared inspector

Press `5` for **Projects**. Spark links Compose working directories, PM2 working
directories, native processes with a project manifest or Git directory, listening
ports, and volume attachments. Configured projects remain visible when stopped.
Directory discovery runs in the background and is cached by process identity;
`F5` rebuilds discovery. Paths from the active Docker context are metadata, and
may not exist on this machine. Unknown volume ownership stays under **Unassigned volumes**.

The project CPU/RAM totals cover native processes and PM2. A process represented
in both lists contributes once. RAM uses native PSS where available, with `~`
for an RSS estimate. `+?` marks a partial total when native/PM2 measurements are
missing. Container RAM is shown separately in Docker and its inspector and is
excluded from native totals; Docker CPU is not currently measured.
`f` favorites a project and keeps it at the top; `/` filters names, directories,
and resources. Column headers and `s` / `r` sort the project table.

`Enter` inspects a project; `F2` inspects the selected resource in any main view.
Large terminals retain the table beside the inspector; smaller terminals show a
single inspector pane. `Esc` returns to the retained table. `Tab` / `Shift+Tab`
or clicking the tab labels changes inspector tabs. In project Details, select a
resource and press `Enter` to inspect it; `Backspace` returns to the project.
`o` opens the selected resource's owning table. In **Ports**, `Enter` jumps directly
to the owning process or container. Native ownership is resolved again on demand,
without the inode cache. Navigation expands the process tree so child owners are visible.
`Esc` cancels that navigation. Missing/exited owners produce a message instead of
selecting a different PID.

### Logs, events, and trends

`l` opens live logs for a container, PM2 app, or native process. Docker/PM2 output
uses a streaming CLI; native logs follow the accessible system journal for that
PID and current boot, starting at its recorded start time. Native stdout pipes
are not read. Native log readers close on kernel process-exit notification
([Linux pidfd](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)); this needs
Linux 5.3 or newer. Journal availability and permissions affect which native logs
can be shown. See [journalctl](https://raw.githubusercontent.com/systemd/systemd/main/man/journalctl.xml).

In Logs, `/` searches, `p` freezes/resumes the displayed buffer, arrows scroll,
and `End` resumes following. Errors are highlighted, long lines are truncated,
and each displayed buffer keeps at most 2,000 lines / 2 MiB. Terminal control
sequences are removed. `y` copies the visible matching log lines or a resource
path; `Y` copies its command. Copy uses OSC 52 and requires terminal clipboard support.
One-shot legacy log dialogs refresh only when you press `F5`.

Events records Docker lifecycle, exit codes, OOM, health, and volume events from
an event stream, plus native/PM2 changes observed in existing snapshots. Streams
start when Projects or an inspector is opened; `F5` on Events reconnects a failed
Docker stream. Native disappearance does not prove a crash or identify an exit
code. Trends uses existing CPU/RAM snapshots; blank intervals have no samples.
History is session-local and bounded to 2,000 events and 300 samples per resource.
There are no new recurring command queries or filesystem scan loops.

### Project storage cleanup

Choose **Storage** (`v`) to load volume sizes, ownership, and activity on demand.
`F5` refreshes the audit. `Space` selects volumes; `Delete` prepares a fresh review
of the complete selection. The review lists exact affected container IDs, images,
status, projects, and their other volumes that will be retained. `y` confirms;
`n` / `Esc` cancels review. The batch checks every selected volume's references
again before its first mutation and stops if they changed. It removes each reviewed
container once, then the selected volumes, without deleting their other volumes.
Docker can still reject a volume if a new reference appears during removal.
Failures retain confirmed partial progress.

`Esc` during removal returns to browsing while cleanup runs; `F6` reopens progress
or its result. Volume accounting can remain unknown when Docker cannot report it;
container attachment activity is evidence, not a volume file-access timestamp.

### Saved views and dev/prod scripts

Sorts, current filters, favorites, configured projects, and named filters are saved
in `$XDG_CONFIG_HOME/spark/workspace.json` (normally `~/.config/spark/workspace.json`).
`SPARK_CONFIG_DIR` overrides the directory for isolated workspaces. Writes are atomic
with file permissions `0600`; invalid/unsupported files are preserved. `W` opens
named filters, `n` saves the current filter under a name, `Enter` applies one, and
`Delete` removes one.

`C` opens a project editor; enter its name, absolute directory, and executable
script paths. `Tab` changes fields; `Ctrl+S` saves. Paths may be absolute or relative
to the project directory; Spark does not guess script names or evaluate shell text.
For example:

```json
{
  "version": 1,
  "view": "projects",
  "projects": [{
    "name": "my-app",
    "path": "/home/dev/my-app",
    "start_dev": "./start-dev",
    "stop_dev": "./stop-dev",
    "start_prod": "./start-prod",
    "stop_prod": "./stop-prod"
  }]
}
```

`D` starts dev by running **stop_prod → start_dev**; `P` starts prod with
**stop_dev → start_prod**. Both required scripts are checked before execution.
The opposite stop must succeed; it has a 120-second limit. Start scripts may remain
in the foreground. The Run tab streams output and retains failures; `X` cancels
Spark's running script process group. One launch per project runs at a time.
Closing the inspector preserves the job; quitting Spark cancels its owned script
process groups. Docker services already started in detached mode remain under Docker's
control. Resource snapshots show service state separately from script exit status.

## Validation

```bash
cargo test --offline
cargo clippy --offline --all-targets
cargo build --offline
python3 scripts/qa_docker_tui.py target/debug/spark
python3 scripts/qa_workspace_tui.py target/debug/spark
python3 scripts/qa_docker_memory_tui.py target/debug/spark
```

The terminal smoke test uses temporary fake Docker and PM2 CLIs and an owned native
process. It checks slow requests, daemon recovery, native data during a slow PM2
query, keyboard menus, background actions, persistent failures, duplicate
prevention, sorting with stable action targets, volume project directories and
owner navigation, logo animation and help, volume details and attached-container
deletion, navigation, clean exit,
and terminal restoration without changing real containers or PM2 applications.
Set `SPARK_SNAPSHOT_DIR=/tmp/spark-ui` when running the tests to export render
snapshots, including resource layouts from 140×36 down to 30×10.
Column-header tests compare the click regions with the actual rendered headers
at these sizes, including compact dialogs, Node.js Processes and PM2 tabs, and hidden or
stale layouts.

The workspace terminal test additionally checks project relationships, saved views,
inspector panes and tab controls, log search/pause/copy, event causes, native trends,
script mode ordering, foreground cancellation and failed-stop protection, batch
review/cancel/reference-change protection, background cleanup, retained other volumes,
port ownership, native log shutdown on process exit, and relaunch persistence. Docker,
PM2, and journal commands are fixtures. Native navigation uses an owned temporary
localhost listener; all executable scripts and processes belong to the test.

The Docker memory terminal test drives a fake stats stream through live updates,
numeric sorting, stable inspector selection, stopped containers, stream failure,
cached readings, explicit reconnection, compact layouts, and clean shutdown.
