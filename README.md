# Email

![email in a terminal](preview.png)

## What it is

A terminal client for standard IMAP and SMTP accounts. It reads, sends, searches,
and manages mail without a browser or a TUI.

## Why this exists

A browser-based mail client is a separate application inside a browser: a tab, a
login session, and a mouse-first interface stand between you and a single
message. Terminal mail has no such layer. It runs in the shell you are already
using, works over SSH, keeps the keyboard as the primary interface, and exposes
messages to scripts instead of only through a web UI.

## How it works

1. Accounts live in `~/.config/email/config.json`; passwords live in the
   operating system's keyring.
2. IMAP handles listing, searching, reading, folders, flags, moves, and trash.
   TLS is verified by default.
3. `list` and `search` remember the mailbox they used, so follow-up commands use
   the same one.
4. Reading marks a message as seen. `--no-mark-read` opts out.
5. MIME is parsed locally, including charsets, encoded headers, and attachments.
6. Sending and replying go through SMTP. Bcc is used for the envelope only.
7. Provider presets cover common hosts and folders; Gmail's localized folders
   are discovered through IMAP special-use flags.

## Usage

### Gmail setup

1. Enable 2-Step Verification.
2. Create an app password at <https://myaccount.google.com/apppasswords>.
3. Add the account:

```bash
email account add --email you@gmail.com
email account test
email list
```

Use the 16-character app password, not the normal Google password. If the app
password page is unavailable, 2FA is off, 2FA only uses security keys, Advanced
Protection is enabled, or a Workspace administrator disabled app passwords.

### Commands

| Command | Action |
| --- | --- |
| `email account add` | Add or update an account |
| `email account list` | Show configured accounts |
| `email account test` | Check IMAP and SMTP |
| `email account password` | Store a new password in the keyring |
| `email account remove` | Remove an account and its keyring entry |
| `email list` | List the current mailbox |
| `email search` | Gmail search syntax, or IMAP criteria elsewhere |
| `email read` | Read a message and list attachments |
| `email send` | Send mail, with HTML and attachments |
| `email reply` | Reply or reply-all |
| `email attachments` | List attachment numbers |
| `email attachment` | Download one attachment |
| `email folders` | List, create, rename, or delete folders |
| `email move` | Move messages to another folder |
| `email archive` | Remove messages from the inbox |
| `email trash` | Move messages to trash |
| `email mark` | Mark read/unread or star/unstar |

### Examples

```bash
email list -n 20
email list -m Archive
email list --unread

email search 'from:abertay has:attachment'
email search --from alice --subject invoice --since 2026-01-01

email read 4821
email read 4821 --html
email read 4821 --format json
email read 4821 --format raw

email attachments 4821
email attachment 4821 1 --output ~/Downloads/invoice.pdf

email send --to alice@example.com -s 'Hello' -b 'From the terminal.'
email reply 4821 --all -b 'Sounds good.'
```

Multiple UIDs can be passed with spaces or commas:

```bash
email trash 4821 4822 4823
email trash 4821,4822,4823
```

Gmail searches use `[Gmail]/All Mail` by default. Other mailboxes can be
selected with `--mailbox`.

## Install

**Requirements:** Rust 1.85+, a C toolchain, OpenSSL, and an unlocked Secret
Service / KWallet / Keychain / Credential Manager for password storage.

```bash
cargo install --git https://github.com/batusaribay/email.git --locked
```

## Uninstall

```bash
email account remove   # removes the account and its keyring entry
cargo uninstall email
rm -rf ~/.config/email
```

## Security

- Passwords are stored in the OS keyring, never in `config.json`. The config
  directory is `0700` and the file is written atomically with mode `0600`.
- Older plaintext passwords are migrated to the keyring on first run.
- TLS certificates are verified. `none` is available only as an explicit choice
  for local relays.
- Bcc addresses are omitted from message headers.
- Outgoing headers are stripped of CR/LF, and attachment filenames are
  sanitized before use as local paths.
- Text output strips terminal control characters.
- No telemetry, analytics, or background polling.

An app password grants full mailbox access. Revoke it from your provider if a
machine is lost.

## License

This project is licensed under the [MIT License](LICENSE).
