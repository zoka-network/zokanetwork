#!/usr/bin/env python3
"""Publish a static, read-only view of the chain for the web explorer.

The explorer used to reach the chain through a serverless proxy pointing at a
central RPC host. That host no longer exists, and the proxy is what exhausted
the hosting plan. This script replaces it: it reads the operator's own node and
writes plain JSON files whose paths mirror the node's API, so the explorer can
be pointed at a bucket instead of a server by editing one config value.

What this is NOT: an authority. Nothing here is trusted by the network. It is a
published view, and the explorer shows how old it is so a stalled publisher
looks stalled instead of looking live.

Usage:
    python publish.py --out ./out
    python publish.py --out ./out --node http://127.0.0.1:3000
"""

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_NODE = "http://127.0.0.1:3000"
TIMEOUT_SECS = 20

# The node rate-limits its own API, and publishing the whole chain is thousands
# of requests — enough to trip it after roughly five hundred. A 429 is the node
# asking to be asked more slowly, so it is waited out rather than counted as a
# failure; without this the publish stopped at block 550 and still reported
# success.
RATE_LIMIT_RETRIES = 8
RATE_LIMIT_BACKOFF_SECS = 2
RATE_LIMIT_MAX_BACKOFF_SECS = 30

# The explorer walks history in 1 000-block windows and builds the URLs itself,
# so this has to match its CHUNK exactly or the files it asks for won't exist.
HISTORY_CHUNK = 1000

# The dashboard also asks for windows anchored to the *current* height rather
# than to a fixed grid: the newest few blocks, and a longer look-back for recent
# private activity. Those URLs change with every block, so they are generated
# from the height being published — which is the same height the explorer reads
# from /chain/height, so the two agree.
#
# These mirror getRecentBlocks(count) and getRecentPrivateActivity's lookback in
# the explorer. If either changes there, it has to change here too, or the
# dashboard asks for files nobody wrote.
RECENT_BLOCKS = 5
PRIVATE_ACTIVITY_LOOKBACK = 64

# How many recent heights to publish those anchored windows for.
#
# The explorer derives the URLs from the height it holds, and that height can
# lag: the browser and the CDN both cache, so a page can still be working from
# the previous publish. Writing only the exact current window means every one
# of those requests 404s until the client happens to refetch — a race that
# repeats on every publish. Covering a span of recent heights costs a handful
# of small files and removes the race entirely.
RECENT_HEIGHT_SPAN = 30


class NodeError(RuntimeError):
    pass


def fetch(node: str, path: str):
    """Read one endpoint, waiting out the node's rate limiter.

    Publishing the whole chain means thousands of requests, which trips the
    node's own rate limit — and a 429 is not a failure, it is the node asking
    to be asked more slowly. Backing off and retrying is the difference between
    a complete chain and one that stops at block 550.
    """
    url = f"{node.rstrip('/')}{path}"
    delay = RATE_LIMIT_BACKOFF_SECS
    for attempt in range(RATE_LIMIT_RETRIES + 1):
        try:
            with urllib.request.urlopen(url, timeout=TIMEOUT_SECS) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            if exc.code == 429 and attempt < RATE_LIMIT_RETRIES:
                time.sleep(delay)
                delay = min(delay * 2, RATE_LIMIT_MAX_BACKOFF_SECS)
                continue
            raise NodeError(f"{path}: {exc}") from exc
        except urllib.error.URLError as exc:
            raise NodeError(f"{path}: {exc}") from exc
        except json.JSONDecodeError as exc:
            raise NodeError(f"{path}: response was not JSON ({exc})") from exc
    raise NodeError(f"{path}: still rate limited after {RATE_LIMIT_RETRIES} retries")


def fetch_optional(node: str, path: str):
    """Fetch an endpoint that an older node may not serve.

    Returns None instead of raising so one missing endpoint degrades a single
    page rather than aborting the whole publish.
    """
    try:
        return fetch(node, path)
    except NodeError:
        return None


def write_json(out_dir: Path, rel_path: str, payload) -> int:
    """Write one object. `rel_path` mirrors the node's API path exactly, so the
    explorer only has to swap its base URL.

    One wrinkle: the API has paths that are both a resource and a prefix —
    `/blocks/range/A/B` returns blocks, while `/blocks/range/A/B/private_tx_hashes`
    also exists. A filesystem cannot have `B` be a file and a directory at once,
    so when that collision comes up the payload goes to `B/index.html`, which is
    what a web server returns for a request to `B` anyway.
    """
    target = out_dir / rel_path.lstrip("/")

    # Any ancestor already written as a file has to become a directory holding
    # its own payload as index.html. Done before mkdir, because mkdir is what
    # fails when the collision is already on disk.
    ancestors = []
    node_path = target.parent
    while node_path != out_dir and node_path != node_path.parent:
        ancestors.append(node_path)
        node_path = node_path.parent
    for ancestor in reversed(ancestors):
        if ancestor.is_file():
            existing = ancestor.read_bytes()
            ancestor.unlink()
            ancestor.mkdir(parents=True, exist_ok=True)
            (ancestor / "index.html").write_bytes(existing)

    target.parent.mkdir(parents=True, exist_ok=True)
    if target.is_dir():
        target = target / "index.html"
    body = json.dumps(payload, separators=(",", ":"), sort_keys=True)
    target.write_text(body, encoding="utf-8")
    return len(body)


def publish(node: str, out_dir: Path) -> dict:
    published_at = int(time.time())

    stats = fetch(node, "/stats")
    sync = fetch(node, "/sync/status")

    height = int(stats.get("height") or sync.get("local_height") or 0)
    if height <= 0:
        raise NodeError("node reported height 0; refusing to publish an empty view")

    # Only publish a node that is actually on the network's tip. A node that is
    # still syncing would otherwise publish a view of the past as if it were
    # current, which is worse than publishing nothing.
    lag = int(sync.get("lag") or 0)
    if sync.get("status") != "caught_up" or lag > 2:
        raise NodeError(
            f"node is not caught up (status={sync.get('status')}, lag={lag}); "
            "refusing to publish a stale view"
        )

    written = {}

    # Mirror the API paths the explorer already calls.
    written["/chain/height"] = write_json(out_dir, "/chain/height", {"height": height})

    stats_payload = dict(stats)
    stats_payload["published_at"] = published_at
    written["/stats"] = write_json(out_dir, "/stats", stats_payload)

    # Dashboard summary. Carries its own timestamp so the explorer can say how
    # old this is rather than implying it is live.
    status = {
        "network": "zoka-mainnet-zenith-v1",
        "height": height,
        "difficulty": stats.get("difficulty"),
        "emission_zka": stats.get("emission_zka"),
        "hashrate_hs": stats.get("hashrate_hs"),
        "connected_peers": stats.get("connected_peers"),
        "miners_online": stats.get("miners_online"),
        "network_version": stats.get("network_version"),
        "last_block_time": stats.get("last_block_time"),
        "published_at": published_at,
    }
    written["/network-status.json"] = write_json(out_dir, "/network-status.json", status)

    # History, in the exact 1 000-block windows the explorer asks for.
    #
    # This is what the scan-key search needs. That search walks the whole chain
    # in the browser, deriving the payment address locally and comparing it
    # against public coinbase records — the key never leaves the machine — so
    # the full history has to exist as files or the search silently stops
    # working. For a miner it is the thing that proves they were paid, which
    # makes it the most useful page on the site.
    #
    # Paths mirror the API exactly, because the explorer builds these URLs
    # itself from the height it just read.
    for from_h in range(1, height + 1, HISTORY_CHUNK):
        to_h = min(from_h + HISTORY_CHUNK - 1, height)

        coinbases = fetch_optional(node, f"/private/coinbases/range/{from_h}/{to_h}")
        if coinbases is not None:
            rel = f"/private/coinbases/range/{from_h}/{to_h}"
            written[rel] = write_json(out_dir, rel, coinbases)

        hashes = fetch_optional(node, f"/blocks/range/{from_h}/{to_h}/private_tx_hashes")
        if hashes is not None:
            rel = f"/blocks/range/{from_h}/{to_h}/private_tx_hashes"
            written[rel] = write_json(out_dir, rel, hashes)

    # Individual blocks, so a visitor can actually open one.
    #
    # Without these the dashboard works but every block is a dead link, which
    # is not really an explorer. Measured on mainnet: ~450 bytes per block and
    # ~140 for its transaction list, so the whole chain is about 6 MB.
    #
    # Written incrementally. A block, once written, never changes — its content
    # is fixed by consensus — so re-fetching twenty thousand of them every run
    # would be pure waste. Only what is missing gets fetched, which after the
    # first run is just the new tail.
    fetched = 0
    skipped = 0
    failed = 0
    first_failure = None
    for h in range(1, height + 1):
        rel = f"/blocks/{h}"
        target = out_dir / rel.lstrip("/")
        if target.exists() or (target / "index.html").exists():
            skipped += 1
            continue
        try:
            block = fetch(node, rel)
        except NodeError as exc:
            failed += 1
            if first_failure is None:
                first_failure = f"{rel}: {exc}"
            continue
        write_json(out_dir, rel, block)
        txs = fetch_optional(node, f"/blocks/{h}/transactions")
        if txs is not None:
            write_json(out_dir, f"/blocks/{h}/transactions", txs)
        fetched += 1

    # Report, rather than swallow. A silent skip here means blocks a visitor
    # can never open, and the publish would still look successful — which is
    # exactly how the earlier gaps went unnoticed.
    if failed:
        print(
            f"warning: {failed} block(s) could not be fetched; first was {first_failure}",
            file=sys.stderr,
        )
    written[f"/blocks/* (new={fetched} cached={skipped} failed={failed})"] = fetched

    # Private transactions by hash, for the scan-key search and the record
    # lookup. Hashes only identify a transaction; they reveal neither parties
    # nor amounts, which stay encrypted on the chain itself.
    #
    # Collected from the per-block hash lists already published above, so this
    # covers exactly what the explorer can ask about. Measured on mainnet:
    # around 250 across the whole chain, so this is cheap.
    tx_hashes = set()
    for from_h in range(1, height + 1, HISTORY_CHUNK):
        to_h = min(from_h + HISTORY_CHUNK - 1, height)
        window = out_dir / f"blocks/range/{from_h}/{to_h}/private_tx_hashes"
        if not window.is_file():
            continue
        try:
            data = json.loads(window.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        for block in data.get("blocks", []) or []:
            for tx_hash in block.get("private_tx_hashes", []) or []:
                if tx_hash:
                    tx_hashes.add(tx_hash)

    tx_written = 0
    for tx_hash in sorted(tx_hashes):
        rel = f"/private/tx/{tx_hash}"
        if (out_dir / rel.lstrip("/")).exists():
            continue
        payload = fetch_optional(node, rel)
        if payload is not None:
            write_json(out_dir, rel, payload)
            tx_written += 1
    written[f"/private/tx/* (new={tx_written} total={len(tx_hashes)})"] = tx_written

    # Mempool counts. Bare numbers — no amounts, no parties. These are live
    # figures being served from a periodic snapshot, so they are the most
    # approximate thing here; the published_at timestamp is what makes that
    # legible rather than misleading.
    for path in ("/mempool/metrics", "/private/pending"):
        payload = fetch_optional(node, path)
        if payload is not None:
            written[path] = write_json(out_dir, path, payload)

    # Windows anchored to a height, for the dashboard's newest-blocks list and
    # its recent-activity feed. Published for a span of recent heights, not just
    # the current one, because a client working from a cached height would
    # otherwise ask for a window that was never written.
    for anchor in range(max(1, height - RECENT_HEIGHT_SPAN + 1), height + 1):
        recent_from = max(1, anchor - RECENT_BLOCKS + 1)
        activity_from = max(1, anchor - PRIVATE_ACTIVITY_LOOKBACK + 1)
        for rel in (
            f"/blocks/range/{recent_from}/{anchor}",
            f"/blocks/range/{recent_from}/{anchor}/private_tx_hashes",
            f"/blocks/range/{activity_from}/{anchor}/private_tx_hashes",
        ):
            payload = fetch_optional(node, rel)
            if payload is not None:
                written[rel] = write_json(out_dir, rel, payload)

    return {"height": height, "published_at": published_at, "written": written}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node", default=DEFAULT_NODE, help="local node API base URL")
    parser.add_argument("--out", required=True, help="output directory")
    args = parser.parse_args()

    out_dir = Path(args.out)
    try:
        result = publish(args.node, out_dir)
    except NodeError as exc:
        print(f"publish failed: {exc}", file=sys.stderr)
        return 1

    print(f"published height {result['height']} to {out_dir}")
    for path, size in sorted(result["written"].items()):
        print(f"  {path:28} {size:>7} B")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
