#!/usr/bin/env python3
"""budget-notify — tenant-facing half of topic-budget reconciliation.

Design: docs/design/zenoh-cardinality-budget-and-lazy-crawler.md §2c
(ADR 1104 for the lifecycle-class tone rules). The router can never refuse a
publisher in-band (brokerless; F-177 silent denial; no NACK on put), and
topic minting is usually unattended software — so tenant feedback is
out-of-band, state-transition driven, one-time per stage.

Reads the crawler's band JSON (already on disk every cycle — no zenoh
client, no fabric identity, no new image), classifies each granted owner
against three stages, and sends AT MOST ONE email per
(owner, grant-generation, stage):

  approaching  observed >= warn_fraction * granted   (30% => 30 on the 100 block)
  at_cap       observed >= granted
  over         budget_state == "exception"            (observed > granted)

A raised allowance changes the grant-generation and re-arms all stages.
Address resolution: owner anchor -> assigned_to handle (portal registry,
read-only) -> email (ansible-rendered manifest projection, then the Kratos
identity whose email local-part is the handle). An owner that resolves to no
email (synths, infra stores) is logged and skipped — the operator
bus/alerting already covers those; machinery never spams a rig.
Lifecycle-class hook (ADR 1104): when the slots row carries a class other
than real, route to log only. Absent column/value => real (fail-safe).

State ledger is a plain JSON file; atomic replace. --dry-run prints
instead of sending. Three honesty rules govern the ledger, because every
entry it holds is a one-time notice that can never be re-sent:

  - written after EACH successful send, not once at the end, so an SMTP
    failure mid-batch never re-mails the owners already delivered to;
  - a resolver ERROR (registry unreadable, Kratos down) is NOT ledgered —
    only a genuine no-address is. A dependency hiccup must not silently
    consume a tenant's one notice;
  - a band JSON older than --max-sample-age means a dead crawler, and the
    whole run declines: stale verdicts are worse than late ones.
"""
import argparse
import json
import math
import os
import smtplib
import sqlite3
import subprocess
import sys
import time
import urllib.request
from email.message import EmailMessage

STAGES = ("approaching", "at_cap", "over")


def load_json(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except Exception:
        return default


def stage_for(observed, granted, budget_state, warn_fraction):
    if granted <= 0:
        return None
    if budget_state == "exception":
        return "over"
    if observed >= granted:
        return "at_cap"
    if observed >= math.ceil(warn_fraction * granted):
        return "approaching"
    return None


# The portal registry on Postgres (ADR 1165 step A, portal_store: postgres):
# read through the Postgres container's local socket, which pg_hba trusts, in a
# read-only session — the same access root had to registry.db. Same mechanism
# as ops/scripts/portal_registry_pg.py; copied here because the role deploys
# this script as a single file.
def pg_query(pg, sql, params):
    """Rows of `sql` (one SELECT, :'name' parameters) as dicts. Raises
    RuntimeError when the registry could not be read at all."""
    argv = ["docker", "exec", "-i",
            "-e", "PGOPTIONS=-c default_transaction_read_only=on",
            pg["container"], "psql", "-X", "-q", "-A", "-t",
            "-v", "ON_ERROR_STOP=1", "-U", pg["user"], "-d", pg["db"]]
    for k, v in params.items():
        argv += ["-v", "%s=%s" % (k, v)]
    stmt = "SELECT coalesce(json_agg(t), '[]'::json) FROM (%s) t;\n" % sql
    try:
        out = subprocess.run(argv, input=stmt, capture_output=True, text=True,
                             timeout=60, check=False)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise RuntimeError(str(e))
    if out.returncode != 0:
        raise RuntimeError("psql exit %d: %s" % (out.returncode, out.stderr.strip()))
    rows = json.loads(out.stdout.strip() or "[]")
    if not isinstance(rows, list):
        raise RuntimeError("expected a JSON array from psql")
    return rows


def resolve_owner_pg(pg, owner):
    """resolve_owner() against the Postgres registry. lifecycle_class is part
    of the step A schema, so there is no column probe."""
    try:
        rows = pg_query(pg, "SELECT assigned_to, lifecycle_class FROM slots "
                            "WHERE slot_id = :'owner'", {"owner": owner})
    except (RuntimeError, ValueError) as e:
        print("registry unreadable (%s/%s): %s" % (pg["container"], pg["db"], e), file=sys.stderr)
        return None, "real", False
    if not rows:
        return None, "real", True
    return rows[0]["assigned_to"], (rows[0]["lifecycle_class"] or "real"), True


def resolve_owner(db_path, owner):
    """(handle, lifecycle_class, ok) for a slot anchor.

    lifecycle_class column may not exist yet (ADR 1104 migration pending) —
    treat missing column/value as 'real', the fail-safe class.

    `ok` separates the two outcomes the caller must never conflate: False =
    the registry could not be read (locked, missing, corrupt) and this owner's
    address is UNKNOWN; True with handle=None = the registry was read and
    genuinely has no row. Only the latter may be ledgered.

    No onboard_proposals tier: that table belongs to the console's store, is
    frozen on ZAGROS (4 of 141 assigned handles) and absent on a fresh goatnet,
    where a bare except silently swallowed the failure. The Kratos tier below
    reads the live identity store, which is authoritative for email anyway.
    """
    try:
        db = sqlite3.connect("file:%s?mode=ro" % db_path, uri=True)
    except Exception as e:
        print("registry unreadable (%s): %s" % (db_path, e), file=sys.stderr)
        return None, "real", False
    try:
        cols = [c[1] for c in db.execute("PRAGMA table_info(slots)")]
        want = "lifecycle_class" if "lifecycle_class" in cols else "NULL"
        row = db.execute(
            "SELECT assigned_to, %s FROM slots WHERE slot_id = ?" % want, (owner,)
        ).fetchone()
        if not row:
            return None, "real", True
        return row[0], (row[1] or "real"), True
    except Exception as e:
        print("registry query failed for %s: %s" % (owner[:12], e), file=sys.stderr)
        return None, "real", False
    finally:
        db.close()


def kratos_email(handle, admin_url):
    """(email, ok) — the identity whose email LOCAL-PART equals the handle.

    That convention is how the onboard-runner mints portal-only identities
    (no manifest, no proposals row — the
    Kratos identity's email local-part IS the handle). Requires a UNIQUE
    match; zero or ambiguous resolves to "" (honest skip, never a guessed
    address). admin_url supports "docker:<container>:<port>" — resolved to the
    container's current IP at each run, since Kratos' compose-network IP
    changes across recreations.

    `ok` is False only when the lookup could not be performed (Kratos down,
    container gone). A successful lookup that matched nobody is ok=True with
    an empty address — a genuine no-address, safe to ledger. The tier being
    disabled (empty admin_url) is likewise ok=True: not consulting Kratos is a
    configuration choice, not an outage.
    """
    if not handle or not admin_url:
        return "", True
    if admin_url.startswith("docker:"):
        try:
            _, container, port = admin_url.split(":", 2)
            ip = subprocess.check_output(
                ["docker", "inspect", container, "--format",
                 "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}"],
                timeout=10).decode().split()[0]
            admin_url = "http://%s:%s" % (ip, port)
        except Exception as e:
            print("kratos container lookup failed: %s" % e, file=sys.stderr)
            return "", False
    try:
        req = urllib.request.Request(admin_url.rstrip("/") + "/admin/identities?per_page=500")
        with urllib.request.urlopen(req, timeout=10) as resp:
            ids = json.load(resp)
    except Exception as e:
        print("kratos identity fetch failed: %s" % e, file=sys.stderr)
        return "", False
    matches = []
    for ident in ids:
        email = ((ident.get("traits") or {}).get("email") or "").strip()
        if email and "@" in email and email.split("@", 1)[0] == handle:
            matches.append(email)
    return (matches[0] if len(set(matches)) == 1 else ""), True


def compose(owner, handle, stage, observed, granted, cfg):
    ladder = (
        "Allowances up to 1,000 are sponsor-approved; beyond that needs "
        "platform-admin sign-off. To request more, reply to this email with a "
        "sentence on what's growing."
    )
    nudge = (
        "Before requesting more: growth in the number of distinct topic NAMES "
        "is usually better carried as message content on a stable topic — see "
        "the naming guide: %s (mesh-internal)." % cfg["best_practices_url"]
    )
    if stage == "approaching":
        subject = "[%s] heads-up: your namespace is at %d of %d topics" % (
            cfg["site"], observed, granted)
        lead = (
            "Your namespace currently holds %d distinct topics of an agreed "
            "allowance of %d. Nothing is wrong and nothing is blocked — this "
            "is a one-time heads-up while there is plenty of headroom." % (observed, granted)
        )
    elif stage == "at_cap":
        subject = "[%s] your namespace has reached its %d-topic allowance" % (
            cfg["site"], granted)
        lead = (
            "Your namespace now holds %d distinct topics, which is your agreed "
            "allowance. Existing topics keep working; publishing to new topic "
            "names past the allowance is flagged to the operators." % observed
        )
    else:
        subject = "[%s] your namespace is over its %d-topic allowance" % (
            cfg["site"], granted)
        lead = (
            "Your namespace holds %d distinct topics against an agreed "
            "allowance of %d. Existing data keeps flowing; the overage is "
            "flagged to the operators, who will reach out about either a "
            "bigger allowance or reshaping the keys." % (observed, granted)
        )
    body = (
        "Hello %s,\n\n"
        "An operational note about your namespace on the %s data fabric.\n\n"
        "%s\n\n"
        "The fabric runs per-tenant topic budgets: every tenant has an agreed "
        "ceiling on distinct topic names under their namespace, measured "
        "continuously. %s\n\n%s\n\n— %s operations\n"
    ) % (handle or owner, cfg["site"], lead, ladder, nudge, cfg["site"])
    return subject, body


def save_state(path, state):
    """Atomic-replace the ledger. Called after every successful send, so a
    crash or SMTP failure later in the batch cannot un-record what already
    went out (the duplicate-mail half of the one-time guarantee)."""
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(state, f, indent=1, sort_keys=True)
    os.replace(tmp, path)


def write_metrics(path, fields, labelled=()):
    """Node-exporter textfile metrics: did this run happen, and did it do
    anything. Best-effort — a metrics-write failure must never cost a send.
    The OnFailure unit writes the same file with up=0 when the run crashes
    before reaching here, so the file always reflects the latest attempt."""
    if not path:
        return
    lines = []
    for name, help_text, value in fields:
        lines.append("# HELP %s %s" % (name, help_text))
        lines.append("# TYPE %s gauge" % name)
        lines.append("%s %s" % (name, value))
    for name, help_text, label, value in labelled:
        lines.append("# HELP %s %s" % (name, help_text))
        lines.append("# TYPE %s gauge" % name)
        lines.append('%s{reason="%s"} %s' % (name, label, value))
    try:
        tmp = "%s.tmp.%d" % (path, os.getpid())
        with open(tmp, "w") as f:
            f.write("\n".join(lines) + "\n")
        os.chmod(tmp, 0o644)
        os.replace(tmp, path)
    except Exception as e:
        print("metrics write failed (%s): %s" % (path, e), file=sys.stderr)


def send(cfg, to, subject, body):
    msg = EmailMessage()
    msg["From"] = cfg["smtp_from"]
    msg["To"] = to
    msg["Subject"] = subject
    msg.set_content(body)
    with smtplib.SMTP(cfg["smtp_host"], int(cfg["smtp_port"]), timeout=20) as s:
        s.starttls()
        s.login(cfg["smtp_username"], cfg["smtp_password"])
        s.send_message(msg)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fabric", required=True, help="crawler band JSON (zenoh-fabric.json)")
    ap.add_argument("--state", required=True, help="dedupe ledger JSON path")
    ap.add_argument("--registry", default="", help="portal registry.db (read-only; sqlite store)")
    ap.add_argument("--registry-store", choices=("sqlite", "postgres"), default="sqlite",
                    help="where the portal registry lives (the goatnet's portal_store)")
    ap.add_argument("--pg-container", default="postgres-a")
    ap.add_argument("--pg-db", default="portal_registry")
    ap.add_argument("--pg-user", default="portal")
    ap.add_argument("--emails", required=True, help="handle->email projection JSON")
    ap.add_argument("--site", default="zagros")
    ap.add_argument("--best-practices-url", required=True)
    ap.add_argument("--warn-fraction", type=float, default=0.3)
    ap.add_argument("--max-sample-age", type=int, default=600,
                    help="decline the run when the band JSON's sampled_at is older than "
                         "this many seconds (default 600, mirroring the "
                         "ZenohFabricSamplerStale rule); 0 disables the check")
    ap.add_argument("--metrics-out", default="",
                    help="node-exporter textfile path for this run's outcome "
                         "(empty disables)")
    ap.add_argument("--kratos-admin-url", default=os.environ.get("KRATOS_ADMIN_URL", ""),
                    help="Kratos admin base URL for the second email tier "
                         "(http://host:4434 or docker:<container>:<port>); empty disables")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    if a.registry_store == "sqlite" and not a.registry:
        ap.error("--registry is required with --registry-store sqlite")
    pg = {"container": a.pg_container, "db": a.pg_db, "user": a.pg_user}

    now = int(time.time())
    fabric = load_json(a.fabric, {})
    sampled_at = int(fabric.get("sampled_at") or 0)
    band_age = now - sampled_at if sampled_at > 0 else -1

    def decline(reason, message):
        print(message)
        write_metrics(a.metrics_out, [
            ("goat_budget_notify_up", "1 if the last run completed (declining to mail is a completed run).", 1),
            ("goat_budget_notify_last_run_timestamp_seconds", "Unix time of the last completed run.", now),
            ("goat_budget_notify_band_age_seconds", "Age of the crawler band JSON this run read (-1 = no sampled_at).", band_age),
            ("goat_budget_notify_sends", "Notifications sent this run.", 0),
            ("goat_budget_notify_send_failures", "Sends that failed this run (retry next run; not ledgered).", 0),
            ("goat_budget_notify_deferred", "Owners skipped this run because a resolver was unavailable (not ledgered).", 0),
        ], [("goat_budget_notify_declined", "1 when the run declined to mail, labelled by reason.", reason, 1)])
        return 0

    if fabric.get("budget_source_state") != "live":
        # Stale/absent allowances => stale verdicts; never notify off them.
        return decline("budget_source_not_live",
                       "budget source not live (%s); no notifications"
                       % fabric.get("budget_source_state"))
    # A frozen band JSON keeps budget_source_state=="live" forever, so the
    # source-state ladder alone cannot catch a dead crawler — only the sample
    # age can. Same 600 s threshold as ZenohFabricSamplerStale, so the alert
    # and the notifier agree on what "current" means.
    if band_age < 0:
        return decline("no_sampled_at",
                       "band JSON carries no sampled_at; declining to mail off an undateable sample")
    if a.max_sample_age > 0 and band_age > a.max_sample_age:
        return decline("stale_sample",
                       "band JSON is %ds old (limit %ds) — the crawler is not sampling; "
                       "declining to mail stale verdicts" % (band_age, a.max_sample_age))
    owners = fabric.get("owners_budget") or []
    emails = load_json(a.emails, {})
    state = load_json(a.state, {})
    cfg = {
        "site": a.site,
        "best_practices_url": a.best_practices_url,
        "smtp_host": os.environ.get("SMTP_HOST", ""),
        "smtp_port": os.environ.get("SMTP_PORT", "587"),
        "smtp_username": os.environ.get("SMTP_USERNAME", ""),
        "smtp_password": os.environ.get("SMTP_PASSWORD", ""),
        "smtp_from": os.environ.get("SMTP_FROM", ""),
    }
    if not a.dry_run and not (cfg["smtp_host"] and cfg["smtp_from"]):
        print("SMTP unconfigured; refusing live run (use --dry-run)", file=sys.stderr)
        return 2

    sends = failures = deferred = 0
    for row in owners:
        owner = row.get("owner", "")
        granted = int(row.get("granted") or 0)
        observed = int(row.get("stored_keys") or 0)
        stage = stage_for(observed, granted, row.get("budget_state"), a.warn_fraction)
        if not stage:
            continue
        gen = "%s@%d" % (owner, granted)  # grant-generation: allowance change re-arms
        done = state.get(gen, [])
        # One send per stage; a higher stage doesn't resend lower ones.
        if stage in done:
            continue
        if a.registry_store == "postgres":
            handle, klass, resolved = resolve_owner_pg(pg, owner)
        else:
            handle, klass, resolved = resolve_owner(a.registry, owner)
        # Two tiers: manifest projection (operator correction wins) -> Kratos
        # identity (email local-part == handle).
        to = emails.get(handle or "", "")
        kratos_ok = True
        if resolved and not to:
            to, kratos_ok = kratos_email(handle, a.kratos_admin_url)
        label = "%s stage=%s obs=%d granted=%d handle=%s class=%s" % (
            owner[:12], stage, observed, granted, handle, klass)
        if not resolved or not kratos_ok:
            # A resolver was DOWN, so "no address" is unproven. Ledgering here
            # would spend the owner's single notice on a registry hiccup and
            # they would never hear about their budget again.
            print("DEFER %s (resolver unavailable — not ledgered, retries next run)" % label,
                  file=sys.stderr)
            deferred += 1
            continue
        if klass != "real" or not to:
            # ADR 1104 tone rule + genuinely address-less owners: log only, but
            # still ledger it so the log line is one-time too.
            print("LOG-ONLY %s (no email sent)" % label)
        else:
            subject, body = compose(owner, handle, stage, observed, granted, cfg)
            if a.dry_run:
                print("DRY-RUN would send to %s: %s" % (to, subject))
                continue  # no ledger write on dry-run
            try:
                send(cfg, to, subject, body)
            except Exception as e:
                # Isolated per owner: one refused recipient or a mid-batch SMTP
                # drop must not strand the rest of the batch unledgered.
                print("SEND-FAILED %s: %s (not ledgered, retries next run)" % (label, e),
                      file=sys.stderr)
                failures += 1
                continue
            sends += 1
            print("SENT %s -> %s" % (label, to))
        state.setdefault(gen, []).append(stage)
        state[gen + ".at"] = int(time.time())
        if not a.dry_run:
            # Per send, not per batch: whatever went out is recorded before the
            # next attempt can fail.
            save_state(a.state, state)

    write_metrics(a.metrics_out, [
        ("goat_budget_notify_up", "1 if the last run completed.", 1),
        ("goat_budget_notify_last_run_timestamp_seconds", "Unix time of the last completed run.", int(time.time())),
        ("goat_budget_notify_band_age_seconds", "Age of the crawler band JSON this run read (-1 = no sampled_at).", band_age),
        ("goat_budget_notify_sends", "Notifications sent this run.", sends),
        ("goat_budget_notify_send_failures", "Sends that failed this run (retry next run; not ledgered).", failures),
        ("goat_budget_notify_deferred", "Owners skipped this run because a resolver was unavailable (not ledgered).", deferred),
    ], [("goat_budget_notify_declined", "1 when the run declined to mail, labelled by reason.", "none", 0)])
    return 0


if __name__ == "__main__":
    sys.exit(main())
