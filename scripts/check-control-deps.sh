#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# metralectl-control is embedded in-process by hosts that own their own network
# surface (the metralectl agent, and a managed node's agent). Its normal
# dependency graph therefore carries no web server or HTTP stack, no TLS, no
# service discovery and no telemetry transport: one of those arriving here
# would give every embedding host a second network surface it did not ask for.
#
# The graph is read for every target, so a platform-gated dependency cannot
# hide from a check run on another platform.
set -euo pipefail

forbidden='^(axum|actix-[a-z-]+|warp|hyper|hyper-[a-z-]+|h2|h3|tower|tower-[a-z-]+|reqwest|ureq|tonic|tungstenite|tokio-tungstenite|rustls|rustls-[a-z-]+|tokio-rustls|native-tls|tokio-native-tls|openssl|openssl-sys|aws-lc-rs|aws-lc-sys|rcgen|quinn|mdns-sd|libmdns|zeroconf|opentelemetry|opentelemetry-[a-z-]+|prometheus|metrics-exporter-[a-z-]+)$'

names=$(cargo tree --locked -p metralectl-control -e normal --target all \
    --prefix none --format '{p}' | awk '{print $1}' | sort -u)
# An empty graph means cargo answered something other than a tree; passing on
# it would be a check that inspects nothing.
if ! printf '%s\n' "$names" | grep -qx 'metralectl-control'; then
    echo "metralectl-control is not in its own dependency tree - the check is not working"
    exit 1
fi
echo "metralectl-control: $(printf '%s\n' "$names" | wc -l | tr -d ' ') crates in the normal graph"
hits=$(printf '%s\n' "$names" | grep -E "$forbidden" || true)
if [ -n "$hits" ]; then
    echo "network-surface crates in metralectl-control's dependency graph:"
    printf '%s\n' "$hits" | sed 's/^/  /'
    exit 1
fi
echo "clean"
