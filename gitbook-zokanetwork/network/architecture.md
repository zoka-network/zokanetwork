# Network Architecture

## Bootstrap Nodes

Bootstrap nodes help new nodes discover the network. The current mainnet
network configuration includes an active DigitalOcean bootstrap seed.

## RPC Nodes

Public RPC nodes provide chain data to wallets, explorers, and bootstrap sync.
They are not custody services. Their logging policy should exclude wallet
secrets and private keys.

## Mining Nodes

Mining nodes validate the current chain, select pending work, and produce
RandomX proof-of-work blocks.

## Mobile Clients

ZKAPriv connects to public or custom RPC endpoints, scans chain data, and keeps
wallet secrets on the Android device.

## Desktop Clients

ZSilent Core can supervise local node processes, wallet flows, and mining from
a desktop interface.

## Local Node Preference

Public RPC is useful for bootstrap, explorers, and mobile connectivity, but a
local node gives desktop users more control over sync, mining, and operational
visibility. ZSilent Core is designed around that local-node workflow.

## Transport Notes

The current network configuration documents TCP-based clearnet operation.
Tor/I2P endpoints should be described as active only after the corresponding
network entries and operator procedures are actually in place.
