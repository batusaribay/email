# email

A Rust CLI for email over IMAP and SMTP

![email in a terminal](preview.png)

## Why this exists

Mail clients are graphical, heavy, and hard to script. This one exposes the
everyday mailbox operations as plain commands with stable output, so they work
from a terminal, a pipe, or an agent without a browser.

## How it works

Accounts live in `~/.config/email/config.toml`, while passwords live in the
operating system's keyring. Existing `config.json` files are migrated
automatically. IMAP handles listing, searching, reading, folders,
flags, moves, and trash with TLS verification on by default. `list` and
`search` remember the mailbox they used, so follow-up commands use the same one.
Reading marks a message as seen unless `--no-mark-read` is given.

MIME is parsed locally, including charsets, encoded headers, and attachments.
Sending and replying go through SMTP, with Bcc used only for the envelope.
Provider presets cover common hosts and folders, and Gmail's localized folders
are discovered through IMAP special-use flags. Every listing and message
command can print JSON with `--format json`.

## Usage

```text
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
      --color <WHEN>      Color output: auto, always, or never [default: auto]
  -h, --help              Print help
  -V, --version           Print version
```

```bash
email list -n 20
email search 'from:alice has:attachment'
email read 4821 --format json
email send --to alice@example.com -s 'Hello' -b 'From the terminal.'
email send --to alice@example.com -s 'Hello' --editor
```

## Notes

Gmail requires an app password, not the account password. Multiple UIDs can be
passed with spaces or commas, and `--editor` opens `$VISUAL` or `$EDITOR` for a
multiline body.

## Config

Accounts are stored in `~/.config/email/config.toml`. Manage them with
`email account`; passwords stay in the OS keyring.

## Install

**Requirements:** Rust 1.88+, `git`, a C toolchain, OpenSSL, and an unlocked
Secret Service, KWallet, Keychain, or Credential Manager for password storage.

```bash
cargo install --git https://github.com/batusaribay/email.git --locked
```

## License

This project is licensed under the [MIT License](LICENSE).
