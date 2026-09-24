// SPDX-License-Identifier: AGPL-3.0-only

//! `agent run`, `agent token`, `agent status`.

use crate::cli::AgentRunArgs;
use crate::hostinfo;
use anyhow::{Context, Result};
use metralectl_agent::launcher::DockerLauncher;
use metralectl_agent::server::AgentState;
use metralectl_agent::token;
use metralectl_core::docker::profile::{NvidiaDevices, ROOTLESS_V1};
use metralectl_core::io::{ProcessRunner, StdProcessRunner};
use std::sync::Arc;

/// Whether this machine can actually run a recipe, and why not if it cannot.
///
/// Probed once at startup and reported to the client, so a browser can say
/// "this box cannot launch" instead of offering a button that will fail. A
/// machine that cannot launch is still useful: it can list and inspect.
fn probe_can_launch(runner: &dyn ProcessRunner) -> Result<(), String> {
    match runner.run(&metralectl_agent::fleet::docker_probe_argv()) {
        Ok(out) if out.success() => Ok(()),
        Ok(out) => Err(format!(
            "the docker daemon did not answer: {}",
            out.stderr.trim()
        )),
        Err(e) => Err(format!("docker is not available: {e}")),
    }
}

/// Run the agent in the foreground.
pub fn run(args: &AgentRunArgs) -> Result<()> {
    // First, before anything can print: everything below reports through
    // stderr, and a diagnostic emitted before the redirect is a diagnostic the
    // operator will never find.
    if let Some(path) = &args.log_file {
        metralectl_core::platform::redirect_stdio(path)?;
    }
    let config_dir = hostinfo::config_dir()?;
    // Checked once, up front, so a permission problem is reported in full
    // rather than as whichever of the three state files happened to be touched
    // first — which is how it surfaced as a bare `Permission denied`.
    crate::configdir::ensure_usable(&config_dir)?;
    // Read before anything binds: a bench.yaml that names a missing checkout
    // is a misconfiguration to refuse now, not a job to fail hours later.
    let bench_config = metralectl_agent::bench::BenchConfig::load(&config_dir)?;

    // Acquired only when a browser will actually be served. A node that exists
    // to hold a rank talks to its peers over mutually authenticated TLS and
    // never consults this token; making it a startup requirement meant a
    // worker could not run at all because of a credential it would not use.
    let tok = if args.no_browser {
        None
    } else {
        Some(token::load_or_create(&config_dir)?)
    };
    let runner: Arc<dyn ProcessRunner> = Arc::new(StdProcessRunner);
    // In client mode the refusal is not a probe result that could later change
    // its mind — this agent has no business launching anything, and says so.
    let can_launch = if args.client {
        Err(
            "this agent runs in --client mode: it can discover, pair and monitor, \
             but it will not run a model"
                .to_owned(),
        )
    } else {
        probe_can_launch(runner.as_ref())
    };

    // The fleet view is what makes /control show real machines. It is built
    // from this box's own facts — identity, links, launchability — so a fresh
    // agent shows itself correctly before any peer exists.
    let identity = Arc::new(metralectl_agent::identity::Identity::load_or_create(
        &config_dir,
    )?);
    use metralectl_agent::fabric::FabricProvider as _;
    // Chosen at compile time so the selection policy above the provider stays
    // one shared path. On macOS the Linux provider found no /sys/class/net and
    // enumerated zero interfaces, so a MacBook advertised no address, was
    // undiscoverable, and minted join invitations with an empty command bar.
    #[cfg(target_os = "macos")]
    let fabric = metralectl_agent::fabric::macos::MacFabric::new();
    #[cfg(not(target_os = "macos"))]
    let fabric = metralectl_agent::fabric::linux::LinuxFabric::new();
    // NOT `unwrap_or_default()`. `doctor` learned this already: an
    // enumeration that FAILED is not a machine with no addresses, and the
    // line below makes a claim about the hardware ("no usable network link")
    // that would then be a guess. Keep the two apart — the agent still
    // starts either way, because it can serve this machine's own browser
    // without a cluster link, but it must not say which situation it is in
    // unless it knows.
    let enumerated = fabric.addresses();
    let addresses = enumerated
        .as_ref()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .to_vec();
    let launchability = match &can_launch {
        Ok(()) => metralectl_protocol::fleet::Launchability::yes(),
        Err(why) => metralectl_protocol::fleet::Launchability::no(why.clone()),
    };
    eprintln!("node identity: {}", identity.id().short());
    eprintln!(
        "{}",
        link_line(
            enumerated
                .as_ref()
                .map(|a| a.first().map(|f| (f.addr.to_string(), f.class.label())))
                .map_err(|e| format!("{e:#}"))
        )
    );
    // Real vitals for this machine. Capabilities are probed once here rather
    // than per sample: on a GB10 that probe is what discovers there is no
    // framebuffer to report, and the answer does not change while we run.
    let vitals = metralectl_agent::fleet::SystemVitals::new(
        Arc::clone(&runner),
        hostinfo::cache_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
    );
    eprintln!(
        "telemetry: gpu={} clock={} memory={}",
        vitals.caps().gpu_util,
        vitals.caps().sm_clock,
        if vitals.caps().unified_memory {
            "unified"
        } else {
            "none"
        }
    );

    let beacon_addrs: Vec<std::net::IpAddr> = addresses
        .iter()
        .filter_map(|a| a.addr.parse().ok())
        .collect();

    // Built before anything that holds a handle to it. Both the cluster
    // previewer and the pairing driver dial another machine, and neither can be
    // constructed without a reactor that already exists.
    let rt = tokio::runtime::Builder::new_multi_thread()
        // Two was "ample: this serves one local browser, not a fleet" -- true of
        // the REQUEST rate and false of the work. `Session::handle` and the peer
        // channel's serve_frames shell out to docker synchronously, on the
        // runtime, and a first launch pulls a multi-GB image: that holds a worker
        // for minutes. With two, a second concurrent operation -- another tab, a
        // relayed launch, a rank's Prepare -- took the other one, and the WHOLE
        // agent stalled: vitals, the peer listener, every socket. Other machines
        // then marked this one unreachable mid-launch, which is the opposite of
        // what a launch should do to a fleet view.
        //
        // The real fix is for those calls not to block the runtime at all, but
        // `Session` borrows from the agent state, so it is neither `'static` for
        // `spawn_blocking` nor safe to `block_in_place` while current-thread
        // tests drive this path. Widening the pool does not make blocking calls
        // correct; it makes one long pull cost a fraction of the agent instead of
        // all of it. Eight idle worker threads cost a few hundred KB of stack.
        .worker_threads(8)
        .enable_all()
        .build()
        .context("starting the async runtime")?;

    // Asked once at startup rather than sampled: a machine does not change
    // what accelerator it has while the agent is running, and the fleet view
    // showed an empty string here for every node while the reading it came
    // from already said "NVIDIA GB10".
    let accelerator =
        metralectl_agent::telemetry::accelerator_name(runner.as_ref()).unwrap_or_default();

    let pins = metralectl_agent::identity::PinStore::new(&config_dir);
    // Shared by the listener, which admits a stranger only while it is open,
    // and by the browser verb that opens it. One window, or the gate and the
    // invitation would be talking about different things.
    let joining = Arc::new(metralectl_agent::joining::JoinWindow::default());
    let fleet = metralectl_agent::fleet::LocalFleet::new(
        metralectl_agent::identity::Identity::load_or_create(&config_dir)?,
        pins.clone(),
        metralectl_agent::discovery::local_display_name(),
        addresses.clone(),
        launchability,
        accelerator.clone(),
    )
    .with_vitals(Box::new(vitals))
    .with_running(Box::new(metralectl_agent::fleet::DockerRunning(
        Arc::clone(&runner),
    )))
    // Without this the browser can see peers and not pair with them, which is
    // where the fleet story dead-ended: the dialog existed, the ceremony had
    // nothing to run it.
    .with_pairing(Box::new(crate::peerpairing::RuntimePeerPairing::new(
        Arc::clone(&identity),
        pins.clone(),
    )));

    let fleet = Arc::new(fleet);
    // The Receiver is dropped, deliberately. Holding it for the process
    // lifetime made `events.receiver_count()` permanently non-zero, which
    // killed the "nobody is watching, do not spawn a process to find out"
    // guard in the vitals loop: the agent shelled out to `docker ps` and
    // sampled the GPU every second forever, including under `--no-browser`
    // where nothing can ever subscribe. Every `send` on this channel already
    // ignores its error, so there is nothing to keep alive.
    let (events, _) = tokio::sync::broadcast::channel(256);

    let renderer: Arc<dyn metralectl_agent::rank::RankService> =
        Arc::new(crate::rankservice::LocalRankService::new(
            crate::commands::registry_set()?,
            hostinfo::snapshot()?,
            &ROOTLESS_V1,
            Box::new(NvidiaDevices),
            Box::new(metralectl_core::docker::collective::NcclRoce),
            Arc::clone(&runner),
            crate::rankservice::RankEnvironment {
                can_launch: can_launch.clone(),
                local_addresses: addresses.clone(),
                reachability: Box::new(metralectl_agent::rendezvous::TcpProbe),
                rdma_devices: metralectl_agent::fabric::linux::rdma_devices_by_interface(),
            },
        ));

    // Built before the state so the supervisor task can hold the same driver:
    // a rank that dies after commit has to be noticed by something, and the
    // session only exists while a browser is connected.
    let cluster = Arc::new(metralectl_agent::clusterdriver::ClusterDriver::new(
        Arc::clone(&fleet) as Arc<dyn metralectl_agent::fleet::FleetView>,
        Arc::clone(&renderer),
        Arc::new(crate::peertransport::PeerTransport::new(
            Arc::clone(&identity),
            pins.clone(),
            metralectl_agent::peer::link::SelfIntro::new(can_launch.is_ok(), &accelerator),
        )),
        metralectl_agent::peer::DEFAULT_PEER_PORT,
    ));

    // The peer channel's control core. Its own instances of the same
    // stateless launcher and telemetry the browser state holds, because the
    // listener outlives any session and cannot borrow from `AgentState` —
    // the checks are identical because both are the one `LocalControl`.
    let control_host = Arc::new(metralectl_agent::control::ControlHost::new(
        crate::commands::registry_set()?,
        std::sync::Arc::new(DockerLauncher::new(
            Arc::clone(&runner),
            hostinfo::snapshot()?,
            &ROOTLESS_V1,
            Box::new(NvidiaDevices),
        )),
        Some(Box::new(crate::launchtelemetry::LocalLaunchTelemetry::new(
            Arc::clone(&runner),
            metralectl_agent::launchstats::LaunchSampler::new(Box::new(
                crate::httpscrape::HttpScraper,
            )),
        ))),
        can_launch.clone(),
        accelerator.clone(),
    ));

    let state = Arc::new(AgentState {
        registry: crate::commands::registry_set()?,
        launcher: std::sync::Arc::new(DockerLauncher::new(
            Arc::clone(&runner),
            hostinfo::snapshot()?,
            &ROOTLESS_V1,
            Box::new(NvidiaDevices),
        )),
        token: tok.clone().unwrap_or_default(),
        can_launch: can_launch.clone(),
        accelerator: accelerator.clone(),
        joining: Some(Arc::clone(&joining)),
        port: args.port,
        allow_dev_origins: args.dev_origins,
        fleet: Some(Box::new(FleetHandle(Arc::clone(&fleet)))),
        telemetry: Some(Box::new(crate::launchtelemetry::LocalLaunchTelemetry::new(
            Arc::clone(&runner),
            metralectl_agent::launchstats::LaunchSampler::new(Box::new(
                crate::httpscrape::HttpScraper,
            )),
        ))),
        cluster: Some(Arc::clone(&cluster) as Arc<dyn metralectl_agent::session::ClusterControl>),
        relay: Some(Arc::new(
            metralectl_agent::peer::control::ControlDriver::new(
                Arc::clone(&identity),
                metralectl_agent::identity::PinStore::new(&config_dir),
                Arc::clone(&fleet),
                metralectl_agent::peer::DEFAULT_PEER_PORT,
            ),
        )),
        events: events.clone(),
    });

    use metralectl_agent::session::ClusterControl as _;

    // Watch the cluster stay whole. The settle gate at commit only catches a
    // rank that dies immediately; weights take minutes to load, so a rank that
    // dies during model build passes it and leaves its peers holding GPUs and
    // serving nothing.
    {
        let cluster = Arc::clone(&cluster);
        let events = events.clone();
        rt.spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let cluster = Arc::clone(&cluster);
                // Asking a peer dials the network. That is now awaited rather
                // than run on a blocking thread: `ClusterControl::supervise` is
                // async, so the dial yields the worker instead of occupying one.
                let torn = cluster.supervise().await;
                if let Some(torn) = torn {
                    eprintln!("cluster: {}", torn.why);
                    // And say it where an operator is looking. This used to go
                    // only to the head agent's stderr, so a cluster could die
                    // and the browser went on showing it running -- with a Stop
                    // button that then answered "this agent did not start a
                    // cluster". A fleet view that silently goes stale is worse
                    // than one that says it does not know.
                    for node in torn.nodes {
                        let _ = events.send(metralectl_protocol::msg::ServerMsg::FleetEvent {
                            event: metralectl_protocol::msg::fleet::FleetEvent::AlertRaised {
                                node,
                                alert: metralectl_protocol::fleet::NodeAlert {
                                    kind: metralectl_protocol::fleet::AlertKind::PeerLost,
                                    severity: metralectl_protocol::fleet::Severity::Critical,
                                    detail: torn.why.clone(),
                                },
                            },
                        });
                    }
                }
            }
        });
    }

    // Claim the port BEFORE announcing it. Everything below — the address, the
    // docker status, the pairing token — is a promise about an agent that is
    // about to exist, and on a port conflict it was all printed and then
    // contradicted. The operator was handed a token for nothing.
    let listener = if args.no_browser {
        None
    } else {
        Some(rt.block_on(metralectl_agent::server::bind(args.port))?)
    };

    if args.no_browser {
        // Do not claim a port that was never bound. The whole point of this
        // mode is that there is no browser channel.
        eprintln!("metralectl agent running (peer channel only, no browser port)");
    } else {
        eprintln!("metralectl agent listening on 127.0.0.1:{}", args.port);
    }
    // Client mode is a different kind of agent, not a broken one, so it does
    // not report a docker failure it was never going to use — and it does not
    // repeat the docker-group warning, which would be untrue here.
    if args.client {
        eprintln!("mode: control only — this agent will not run a model");
    } else {
        match &can_launch {
            Ok(()) => eprintln!("docker: ok"),
            Err(why) => eprintln!(
                "docker: unavailable — {why}\n  this agent can list and inspect recipes but not launch them"
            ),
        }
    }
    if args.dev_origins {
        eprintln!("accepting development origins — do not leave this on");
    }
    match &tok {
        Some(t) => eprintln!("\npairing token (paste into the website once):\n  {t}\n"),
        None => eprintln!(
            "\nbrowser channel disabled (--no-browser); no pairing token was created.\n\
             This node is reachable by its paired peers over the peer channel.\n"
        ),
    }
    if args.client {
        eprintln!("This agent does not talk to Docker and cannot start a container.");
        eprintln!("It can discover machines, pair with them, and watch what they are doing.");
    } else {
        eprintln!("This agent talks to Docker. On Linux, membership of the `docker` group is");
        eprintln!("root-equivalent, so anything that can drive this agent can do what you can.");
    }
    eprintln!("Stop it with ctrl-c when you are done.\n");

    rt.block_on(async move {
        // Background work: advertise, listen for peers, sample vitals, age out
        // machines that have gone. Started before serving so the first browser to
        // connect already has a populated fleet.
        let discovery: Option<Arc<dyn metralectl_agent::daemon::DiscoveryPair>> =
            if args.no_discovery {
                eprintln!("discovery disabled; add peers with `metralectl peer add <host>`");
                None
            } else {
                match metralectl_agent::discovery::mdns::MdnsDiscovery::new() {
                    Ok(d) => Some(Arc::new(d)),
                    Err(e) => {
                        eprintln!("discovery unavailable: {e}");
                        None
                    }
                }
            };
        // Serving the peer channel is what turns a pairing into a working
        // link: it is how a peer's real vitals and verified link class arrive,
        // rather than a beacon's unauthenticated word for them.
        // Bench: present only when the operator wrote bench.yaml. Absent is a
        // disabled surface that says so; present-but-wrong refused at
        // startup above, before any port was bound.
        let (bench, bench_disabled) = match &bench_config {
            Ok(cfg) => match metralectl_agent::bench::BenchHost::new(
                cfg.clone(),
                identity.id(),
                Arc::clone(&fleet),
            ) {
                Ok(host) => {
                    eprintln!(
                        "bench: enabled (repo {}, home {}, class {})",
                        cfg.metrale_repo.display(),
                        cfg.metrale_home.display(),
                        cfg.hardware
                    );
                    (Some(host), None)
                }
                Err(e) => {
                    eprintln!("bench: disabled — {e:#}");
                    (None, Some(format!("{e:#}")))
                }
            },
            Err(disabled) => {
                eprintln!("bench: disabled — {}", disabled.0);
                (None, Some(disabled.0.clone()))
            }
        };
        metralectl_agent::daemon::spawn_peer_work(metralectl_agent::daemon::PeerWork {
            fleet: Arc::clone(&fleet),
            identity: Arc::clone(&identity),
            pins,
            events: events.clone(),
            peer_port: args.peer_port,
            rank: Arc::clone(&renderer),
            joining: Arc::clone(&joining),
            accelerator: accelerator.clone(),
            control: control_host,
            bench,
            bench_disabled,
        });

        metralectl_agent::daemon::spawn_all(
            Arc::clone(&fleet),
            events,
            discovery,
            metralectl_agent::discovery::Beacon {
                id: fleet.id(),
                name: metralectl_agent::discovery::local_display_name(),
                peer_port: args.peer_port,
                addresses: beacon_addrs,
                can_launch: can_launch.is_ok(),
                accelerator: accelerator.clone(),
            },
        );

        let Some(listener) = listener else {
            // Nothing to serve; the peer channel and discovery are the point.
            // Park until signalled rather than returning, which would tear the
            // runtime down and take those with it.
            std::future::pending::<()>().await;
            return Ok(());
        };
        metralectl_agent::server::serve_on(state, listener).await
    })
}

mod fleethandle;
mod linkline;

use fleethandle::FleetHandle;
use linkline::link_line;
