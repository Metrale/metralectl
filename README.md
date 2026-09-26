# metralectl

`metralectl` installs, launches and benchmarks the
[Metrale inference engine](https://github.com/Metrale/metrale-inference-alpha)
on NVIDIA DGX Spark (GB10) and other local accelerators.

- **Recipes.** A recipe is one validated model deployment: the checkpoint, the
  container image and the `met serve` settings. The recipes in
  [`recipes/`](recipes/) ship inside the binary, so resolving one needs no
  network request, and a remote registry can supply recipe data but can never
  make `metralectl` run a command. [SECURITY.md](SECURITY.md) explains why.
- **CLI.** `metralectl` reads a recipe and runs the `docker run` it implies, or
  prints it for you to review first.
- **Agent.** `metralectl agent` is a small local service that lets
  [metrale.ai](https://metrale.ai/control) and other paired machines see and
  drive this one. The CLI works without it.

## Install

Linux and macOS:

```sh
curl -fsSL https://metrale.ai/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://metrale.ai/install.ps1 | iex
```

Both installers refuse a download whose checksum is not in the release's
`SHA256SUMS`, and set the agent up to start at login (a `systemd --user` unit,
a LaunchAgent, or a Task Scheduler task). The binary goes to `~/.local/bin`, or
`%LOCALAPPDATA%\Programs\metralectl` on Windows. The environment variables
`METRALECTL_INSTALL_DIR` (another directory), `METRALECTL_VERSION=<tag>` (pin
a release) and `METRALECTL_NO_AGENT=1` (skip the agent) change that. On Linux
and macOS, `curl -fsSL https://metrale.ai/install.sh | sh -s -- --uninstall`
removes the binary and the agent service.

With [uv](https://docs.astral.sh/uv/), no install step:

```sh
uvx metralectl list
```

`uv tool install metralectl` puts it on your PATH, and `cargo install metralectl`
builds it from crates.io.

## Quick start

```sh
metralectl list                                   # the recipes this build can launch
metralectl show qwen3.6-35b-a3b-fp8-mtp           # what one recipe does
metralectl run qwen3.6-35b-a3b-fp8-mtp --print    # the docker command, without running it
metralectl run qwen3.6-35b-a3b-fp8-mtp            # serve it
metralectl doctor                                 # check docker, the agent and this machine
```

`list`, `show` and `run --print` work without Docker. `run` needs Docker and,
for GPU recipes, the NVIDIA container runtime. `metralectl --help` lists every
command.

## Agent, fleets and bench nodes

`metralectl agent token` prints the code that pairs a browser with this
machine. `metralectl agent pair` and `metralectl peer` join machines into a
fleet that can launch multi-node recipes. A machine with a `bench.yaml` becomes
a bench node: a peer holding the separate `bench` grant can submit a
certification gate at a commit, and `metralectl bench` follows the job and
fetches its signed records. [docs/BENCH.md](docs/BENCH.md) covers the setup and
the protocol.

## Licence

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE),
at your option.
