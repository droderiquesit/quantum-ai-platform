#!/usr/bin/env python3
"""Text OWNER_SMS through carrier email-to-SMS gateways (HERMES_MISSION.md §8).

    send-sms.py "message"            send (<=300 chars, plain text)
    send-sms.py --selftest           no network; proves the envelope

Config, all from the environment except the secret, which is a file
(.claude/rules/01-security-and-safety.md: never an env value):
    HERMES_SMTP_HOST (smtp.comcast.net)  HERMES_SMTP_PORT (587)
    HERMES_SMTP_USER                      required; also the From address
    HERMES_SMTP_PASSWORD_FILE             default ~/.config/hermes/smtp_password
    HERMES_SMS_TO                         default 5083179114@vtext.com (Verizon)
"""
import os, smtplib, sys
from email.message import EmailMessage

GATEWAYS = "5083179114@vtext.com"  # Verizon, confirmed by David 2026-10-04

def build(user, to, body):
    if not body or len(body) > 300:
        raise SystemExit("refusing: message must be 1-300 characters")
    m = EmailMessage()
    m["From"], m["To"] = user, to
    m.set_content(body)  # no Subject: gateways prepend it to the text
    return m

def send(body):
    e = os.environ
    user = e.get("HERMES_SMTP_USER") or sys.exit("refusing: HERMES_SMTP_USER is not set")
    pw_file = os.path.expanduser(e.get("HERMES_SMTP_PASSWORD_FILE", "~/.config/hermes/smtp_password"))
    try:
        pw = open(pw_file).read().strip()
    except OSError:
        sys.exit(f"refusing: no SMTP password at {pw_file} (chmod 600 it)")
    msg = build(user, e.get("HERMES_SMS_TO", GATEWAYS), body)
    with smtplib.SMTP(e.get("HERMES_SMTP_HOST", "smtp.comcast.net"), int(e.get("HERMES_SMTP_PORT", "587")), timeout=30) as s:
        s.starttls(); s.login(user, pw); s.send_message(msg)

def selftest():
    m = build("a@example.com", GATEWAYS, "hi")
    assert m["To"] == "5083179114@vtext.com" and "Subject" not in m
    for bad in ("", "x" * 301):
        try: build("a@example.com", GATEWAYS, bad); assert False
        except SystemExit: pass
    print("ok")

if __name__ == "__main__":
    selftest() if sys.argv[1:] == ["--selftest"] else send(" ".join(sys.argv[1:]))
