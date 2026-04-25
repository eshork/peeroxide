#!/usr/bin/env bash
# NAT simulation setup for Docker containers.
# Run inside a container with NET_ADMIN capability.
#
# Creates a MASQUERADE rule that NATs traffic from the private subnet
# through the container's public-facing interface.
#
# Usage: nat-setup.sh <private-subnet> <public-iface>
# Example: nat-setup.sh 10.0.1.0/24 eth0
set -euo pipefail

PRIVATE_SUBNET="${1:?Usage: nat-setup.sh <private-subnet> <public-iface>}"
PUBLIC_IFACE="${2:?Usage: nat-setup.sh <private-subnet> <public-iface>}"

echo "Setting up NAT: ${PRIVATE_SUBNET} → ${PUBLIC_IFACE}"

iptables -t nat -A POSTROUTING -s "$PRIVATE_SUBNET" -o "$PUBLIC_IFACE" -j MASQUERADE
iptables -A FORWARD -i "$PUBLIC_IFACE" -o "$PUBLIC_IFACE" -j ACCEPT

echo 1 > /proc/sys/net/ipv4/ip_forward

echo "NAT setup complete"
