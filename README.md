# email

A Rust CLI for email over IMAP and SMTP

![email in a terminal](preview.png)

## Why it exists

Mail clients are graphical, heavy, and hard to script. This one exposes the
everyday mailbox operations as plain commands with stable output, so they work
from a terminal, a pipe, or an agent without a browser.

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
7. Provider presets cover common hosts and folders. Gmail's localized folders
   are discovered through IMAP special-use flags.
8. Every listing and message command can print JSON with `--format json`.

## Usage

```bash
email [OPTIONS] <COMMAND>

Commands:
  account      Add, inspect, test, or remove mail accounts
  list         List messages in a mailbox
  search       Search messages with Gmail syntax or IMAP criteria
  read         Print a message and its attachment list
  send         Send a new message
  reply        Reply to a message
  forward      Forward one or more messages
  attachments  List attachments on a message
  attachment   Download one attachment from a message
  folders      List, create, rename, or delete mail folders
  move         Move messages to another folder
  archive      Remove messages from the inbox
  trash        Move messages to the trash folder
  mark         Set read or starred flags on messages

Options:
      --config-dir <DIR>  Configuration directory
      --account <EMAIL>   Account email to use
  -h, --help              Print help
  -V, --version           Print version

Examples:
  email list -n 20
  email search 'from:alice has:attachment'
  email read 4821 --format json
  email send --to alice@example.com -s 'Hello' -b 'From the terminal.'
```

## Notes

- Gmail needs an app password: enable 2-Step Verification, create one at
  `https://myaccount.google.com/apppasswords`, then run
  `email account add --email you@gmail.com`, `email account test`, and
  `email list`. Use the 16-character app password, not the Google password.
- Multiple UIDs can be passed with spaces or commas, for example
  `email trash 4821 4822 4823`.
- Forwarding sends one email containing every selected message and keeps the
  original attachments. Gmail searches use `[Gmail]/All Mail` unless
  `--mailbox` says otherwise.
- Passwords are stored in the OS keyring, never in `config.json`. The config
  directory is mode `0700` and the file is written atomically with mode `0600`.
  Old plaintext passwords are migrated on first run.
- TLS certificates are verified. `none` is available only as an explicit choice
  for local relays. Bcc addresses are omitted from message headers, outgoing
  headers are stripped of CR and LF, and attachment filenames are sanitized
  before use as local paths.
- Text output strips terminal control characters. There is no telemetry,
  analytics, or background polling. An app password grants full mailbox
  access, so revoke it at the provider if a machine is lost.

## Install

**Requirements:** Rust 1.88+, `git`, a C toolchain, OpenSSL, and an unlocked
Secret Service, KWallet, Keychain, or Credential Manager for password storage.

```bash
cargo install --git https://github.com/batusaribay/email.git --locked
```

## License

This project is licensed under the [MIT License](LICENSE).
