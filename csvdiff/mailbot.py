"""Email launcher: `csvdiff mail` polls a mailbox, picks up unread messages whose
subject matches, compares the first two CSV attachments and replies with the
HTML report attached.

Subject grammar (case-insensitive, order free):

    csvdiff key=order_id,line_no compare=qty,price ignore=updated_at trim tolerance=0.01
    csvdiff profile=orders
    csvdiff profile=orders a=july.csv b=august.csv     # pick attachments by name

Unknown tokens are ignored, so people can add free text. The same tokens are
also read from the first line of the body if the subject only says "csvdiff".

Configuration lives in the [mail] section of csvdiff.toml (see csvdiff.example.toml).
Passwords come from environment variables, never from the file.
"""
from __future__ import annotations

import email
import email.policy
import gzip
import imaplib
import os
import re
import smtplib
import tempfile
import time
import traceback
from email.message import EmailMessage

from .config import load_config, options_from, parse_list
from .engine import CompareError, compare
from .report import render

TOKEN = re.compile(r"(\w+)=(\"[^\"]*\"|\S+)|(\b(?:trim|ignore_case|empty_is_null)\b)", re.I)
CSV_EXT = (".csv", ".txt", ".tsv")


def parse_directives(text: str) -> dict:
    out = {}
    for k, v, flag in TOKEN.findall(text):
        if flag:
            out[flag.lower()] = True
        else:
            out[k.lower()] = v.strip('"')
    return out


def build_options(directives: dict, profiles: dict):
    profile = profiles.get(directives.get("profile", "")) if directives.get("profile") else None
    if directives.get("profile") and profile is None:
        raise CompareError(f"Profile not found: {directives['profile']}")
    tol = directives.get("tolerance")
    return options_from(profile, {
        "key": parse_list(directives.get("key")), "compare": parse_list(directives.get("compare")),
        "ignore": parse_list(directives.get("ignore")), "trim": directives.get("trim"),
        "ignore_case": directives.get("ignore_case"), "empty_is_null": directives.get("empty_is_null"),
        "tolerance": float(tol) if tol else None, "delimiter": directives.get("delimiter"),
    })


def pick_attachments(msg: EmailMessage, d: dict):
    files = [(p.get_filename(), p.get_payload(decode=True)) for p in msg.iter_attachments()
             if p.get_filename() and p.get_filename().lower().endswith(CSV_EXT)]
    if len(files) < 2:
        raise CompareError(f"Need two CSV attachments, found {len(files)}.")
    by_name = {n.lower(): (n, b) for n, b in files}
    a = by_name.get((d.get("a") or "").lower(), files[0])
    b = by_name.get((d.get("b") or "").lower(), next(f for f in files if f is not a))
    return a, b


def handle_message(msg: EmailMessage, cfg: dict) -> EmailMessage:
    """Run the comparison and build the reply (report or error)."""
    subject = msg.get("Subject", "")
    body = (msg.get_body(preferencelist=("plain",)) or msg).get_content() if msg.is_multipart() or msg.get_content_type() == "text/plain" else ""
    directives = {**parse_directives(body.strip().split("\n", 1)[0] if body else ""), **parse_directives(subject)}
    reply = EmailMessage()
    reply["To"] = msg.get("Reply-To") or msg["From"]
    reply["From"] = cfg["from"]
    reply["Subject"] = "Re: " + subject
    reply["In-Reply-To"] = msg.get("Message-ID", "")
    reply["References"] = msg.get("Message-ID", "")
    try:
        opt = build_options(directives, cfg.get("_profiles", {}))
        if not opt.key:
            raise CompareError("No key given. Put key=col1,col2 or profile=name in the subject.")
        (an, ab), (bn, bb) = pick_attachments(msg, directives)
        with tempfile.TemporaryDirectory() as td:
            pa, pb = os.path.join(td, "A_" + os.path.basename(an)), os.path.join(td, "B_" + os.path.basename(bn))
            open(pa, "wb").write(ab), open(pb, "wb").write(bb)
            result = compare(pa, pb, opt)
            result["meta"]["a"]["name"], result["meta"]["b"]["name"] = an, bn
        c = result["counts"]
        html = render(result).encode("utf-8")
        text = (f"{an} vs {bn}\nKey: {', '.join(opt.key)}\n\n"
                f"A rows: {c['a_rows']:,}   B rows: {c['b_rows']:,}\n"
                f"Matched: {c['matched']:,} (changed {c['changed']:,}, unchanged {c['unchanged']:,})\n"
                f"Added (only in B): {c['added']:,}\nRemoved (only in A): {c['removed']:,}\n"
                f"Duplicate keys: A {c['a_dup_keys']:,}, B {c['b_dup_keys']:,}\n"
                f"Top changed columns: " + (", ".join(f"{x['name']} ({x['changed']:,})" for x in sorted(result['columns'], key=lambda x: -x['changed'])[:5] if x['changed']) or "none")
                + f"\n\nThe full interactive report is attached. {result['meta']['engine']}, {result['meta']['seconds']}s.\n")
        reply.set_content(text)
        name = f"{os.path.splitext(an)[0]}__vs__{os.path.splitext(bn)[0]}.html"
        if len(html) > int(cfg.get("max_report_bytes", 9_000_000)):
            reply.add_attachment(gzip.compress(html), maintype="application", subtype="gzip", filename=name + ".gz")
        else:
            reply.add_attachment(html, maintype="text", subtype="html", filename=name)
    except CompareError as e:
        reply.set_content(f"csvdiff could not run this comparison:\n\n{e}\n\n{__doc__.split('Configuration')[0]}")
    except Exception:
        reply.set_content("csvdiff hit an unexpected error:\n\n" + traceback.format_exc())
    return reply


def _allowed(sender: str, cfg: dict) -> bool:
    allow = cfg.get("allowed_senders")
    if not allow:
        return True
    s = email.utils.parseaddr(sender)[1].lower()
    return any(s == a.lower() or s.endswith(a.lower()) for a in allow)


def process_once(cfg: dict) -> int:
    pat = re.compile(cfg.get("subject_pattern", r"^\s*csvdiff\b"), re.I)
    pw = os.environ.get(cfg.get("imap_password_env", "CSVDIFF_MAIL_PASSWORD"), "")
    done = 0
    with imaplib.IMAP4_SSL(cfg["imap_host"], int(cfg.get("imap_port", 993))) as im:
        im.login(cfg["imap_user"], pw)
        im.select(cfg.get("folder", "INBOX"))
        _, data = im.search(None, "UNSEEN")
        for uid in data[0].split():
            _, raw = im.fetch(uid, "(RFC822)")
            msg = email.message_from_bytes(raw[0][1], policy=email.policy.default)
            if not pat.search(msg.get("Subject", "")) or not _allowed(msg.get("From", ""), cfg):
                continue
            im.store(uid, "+FLAGS", "\\Seen")
            reply = handle_message(msg, cfg)
            send(reply, cfg)
            done += 1
            print(f"replied to {msg['From']}: {msg['Subject']}")
    return done


def send(reply: EmailMessage, cfg: dict):
    pw = os.environ.get(cfg.get("smtp_password_env", cfg.get("imap_password_env", "CSVDIFF_MAIL_PASSWORD")), "")
    with smtplib.SMTP(cfg["smtp_host"], int(cfg.get("smtp_port", 587))) as s:
        if cfg.get("smtp_starttls", True):
            s.starttls()
        if cfg.get("smtp_user", cfg.get("imap_user")):
            s.login(cfg.get("smtp_user", cfg.get("imap_user")), pw)
        s.send_message(reply)


def run(config_path=None, once=False):
    full = load_config(config_path)
    cfg = dict(full.get("mail", {}))
    if not cfg:
        raise CompareError("No [mail] section in csvdiff.toml")
    cfg["_profiles"] = full.get("profiles", {})
    interval = int(cfg.get("poll_seconds", 60))
    print(f"csvdiff mail: watching {cfg['imap_user']} every {interval}s (Ctrl+C to stop)")
    while True:
        try:
            process_once(cfg)
        except (imaplib.IMAP4.error, smtplib.SMTPException, OSError) as e:
            print(f"mail error: {e}")
        if once:
            return 0
        time.sleep(interval)
