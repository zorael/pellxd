# pellxd

Monitor of a **PellX wooden pellets burner**.

Intended to be run on a **Raspberry Pi-equivalent** device connected via GPIO to terminals on the controller board of a PellX burner. Terminals **1** and **2** are electrically connected when the burner is operating normally, and the circuit is broken when it is in an error state (including on power failures). See [page 11 of the manual](https://www.eldkraft.se/uploads/pdf/PELLX/MANUAL-english-PELLX_20KW.pdf#page=12).

A notification is sent when this is detected. The program supports sending such through four different backends; as messages sent to [**Slack**](https://docs.slack.dev/messaging/sending-messages-using-incoming-webhooks) channels, as short emails sent via the free [**Batsign**](https://batsign.me) service, and by invocations of **external commands**.

## tl;dr

```text
Usage: pellxd [OPTIONS]

Options:
      --source <option>     Input source to poll [default: gpio] [possible values: gpio, dummy]
  -c, --config <file>       Specify an alternative configuration file
      --save                Write configuration to disk
      --disable-timestamps  Disable timestamps in terminal output
  -v, --verbose             Print some additional information
  -d, --debug               Print much more additional information
      --dry-run             Perform a dry run, echoing what would be done
```

Create a [configuration file](#configuration) by passing `--save`.

```sh
cargo run -- --save
```

```toml
[monitor]
source = "gpio"

[slack]
enabled = true
urls = ["https://hooks.slack.com/services/..."]
```

```sh
cargo run -- --verbose
```

## toc

- [compilation](#compilation)
  - [cross-compilation](#cross-compilation)
  - [`-j1`](#-j1)
- [configuration](#configuration)
- [strings](#strings)
  - [placeholders](#placeholders)
- [backends](#backends)
  - [slack](#slack)
    - [formatting messages](#formatting-messages)
  - [batsign](#batsign)
    - [formatting mails](#formatting-mails)
  - [external commands](#external-commands)
    - [arguments](#arguments)
  - [`println`](#println)
- [systemd](#systemd)
  - [enable now](#enable-now)
  - [logs](#logs)
- [ai](#ai)
- [todo](#todo)
- [license](#license)

## compilation

This project uses [**Cargo**](https://doc.rust-lang.org/cargo) for compilation and dependency management. Grab it from your repositories, install it via [**Homebrew**](https://formulae.brew.sh/formula/rustup), or download it with the official [`rustup`](https://rustup.rs) installation script.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

You may have to add `$HOME/.cargo/bin` to your `$PATH`.

Use `cargo build` to build the project. This stores the resulting binary as `target/<profile>/pellxd`, where `<profile>` is one of `debug` or `release`, depending on what profile is being built. `debug` is the default; you can make it build in `release` mode with `--release`.

```sh
cargo build
cargo build --release
```

To compile the program and run it immediately, use `cargo run`. If you also want to pass command-line flags to the program, separate them from `cargo run` with [double dashes](https://www.gnu.org/software/bash/manual/html_node/Shell-Builtin-Commands.html) `--`.

```sh
cargo run -- --help
cargo run -- --save
```

You can find the binaries you compile with Cargo in the `target/<profile>/` subdirectory of the project, where `<profile>` is either `debug` or `release`, depending on what profile you built with.

See the [**systemd**](#systemd) section for instructions on how to set it up as a system daemon that is automatically started on boot.

### cross-compilation

Weaker Raspberry Pi models, such as [**the Pi Zero 2W**](https://www.raspberrypi.com/products/raspberry-pi-zero-2-w), can *run* the program but may not have enough memory to compile it with default flags. Your alternatives there are to either build it in serial mode (with `-j1`) on the device itself, or to cross-compile it on a more powerful machine.

Regrettably, manually setting up cross-compilation can be non-trivial. As such, use of one of [`cargo-cross`](https://github.com/cross-rs/cross) or [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) is recommended (but not required). For the latter you need to install a [**Zig**](https://ziglang.org) compiler. Refer to your repositories, alternatively install it via Homebrew (`brew install zig`).

Note that your `$CFLAGS` environment variable must not contain `-march=native` for all dependencies to successfully build.

```sh
cargo install cargo-cross
CFLAGS="-O2 -pipe" cargo cross build --target=aarch64-unknown-linux-gnu
```

```sh
cargo install cargo-zigbuild
CFLAGS="-O2 -pipe" cargo zigbuild --target=aarch64-unknown-linux-gnu
```

This should require upwards of 500 Mb of free system memory, effectively exceeding the total RAM of a Pi Zero 2W.

Both `cargo cross build` and `cargo zigbuild` default to compiling with the `--profile=release` flag, applying some optimizations and considerably lowering the resulting binary file size as compared to when building with `--profile=dev`.

```sh
rsync -avz --progress target/aarch64-unknown-linux-gnu/release/pellxd user@pi:~/
```

Replace `release` with `debug` to transfer the binary of a `--profile=dev` build.

### `-j1`

This will build it in a serial mode (`--jobs=1`), compiling one dependency at a time. Swap is probably still required.

```sh
cargo build -j1
```

Mind that build times will be *very* long. Remember to use a heatsink. (Cross-compilation remains recommended.)

## configuration

Run the program with `--save` to generate a configuration file. By default, this will be stored as `~/.config/pellxd.toml`. You can specify an alternative path with `--config <file>`.

```sh
cargo run -- --config ~/pellxd.toml --save
```

If a filename is not specified, the program will infer a configuration directory in which to place one, based on your user and some environment variables.

- `$PELLXD_CONFIG_ROOT` if set
- `/etc` if run as the root user
- `$XDG_CONFIG_HOME` if set
- `$HOME/.config` if `$HOME` is set
- ...or fail to start if none of the above apply.

The default filename for the configuration file is `pellxd.toml`. If the file does not exist, the program will create it with default values.

```toml
[monitor]
source = "gpio"
loop_interval = "1s"
startup_window = "8m"

[gpio]
pin = 24

[dummy_input]
modulus = 30
threshold = 15
```

Subsequent calls to `--save` will not overwrite existing configuration, but mind that comments will be removed.

## strings

Each notification backend can be configured with its own set of customizable strings, defined in the configuration file to tailor notification messages to your liking.

```toml
[backend.strings]
alert_header = 'PellX burner failure\n'
alert_body = "It went into an error state at {fuzzy_high}."
reminder_header = 'PellX burner still in failure\n'
reminder_body = "It has been in an error state since {fuzzy_high}."
startup_failed_header = "PellX burner startup failed"
startup_failed_body = "It tried to start up but failed at {fuzzy_high}."
startup_success_header = "PellX burner startup succeeded"
startup_success_body = "It successfully started up at {fuzzy_low}."
footer = ""
```

- `alert_header` and `alert_body` are used in the message sent when a failure is first detected.
- `reminder_header` and `reminder_body` are used in messages sent on subsequent iterations of the main loop while the burner is still in an error state.
- `startup_failed_header` and `startup_failed_body` are used in the message sent when a startup attempt is made but fails.
- `startup_success_header` and `startup_success_body` are used in the message sent when a startup attempt is made and succeeds.
- `footer` is appended to the end of all messages.

All of these strings are optional and can be left as an empty string `""` to disable. A message whose header is empty will not be sent, neither will a message whose body ended up empty after composing.

### placeholders

Messages can container certain placeholders that will be replaced with dynamic content when composing the message, such as `{fuzzy_high}` and `{fuzzy_low}` in the examples above.

| Placeholder | Description |
| ----------- | ----------- |
| `{fuzzy_now}` | The current time in a human-friendly format that may be a mixture of date and time, depending how long ago the time was. In the case of the current time, this will always be a timestamp without date. |
| `{time_now}` | The current time in `HH:MM` format. |
| `{date_now}` | The current date in `YYYY-MM-DD` format. |
| `{fuzzy_then}` | The time of the context's `now` field. This is generally the same as the current time, but may be different if the context is from a retry of a previously failed send. |
| `{time_then}` | The time of the context's `now` field, in `HH:MM` format. |
| `{date_then}` | The date of the context's `now` field, in `YYYY-MM-DD` format. |
| `{name}` | The name of the program. |
| `{version}` | The version of the program. |
| `{fuzzy_low}` | The time of the most recent transition to a `LOW` reading, in a human-friendly format. |
| `{fuzzy_high}` | The time of the most recent transition to a `HIGH` reading, in a human-friendly format. |
| `{fuzzy_state_change}` | The time of the most recent transition to either a `LOW` or `HIGH` reading, in a human-friendly format. |

## backends

Notifications can be sent as messages to **Slack** channels, as short emails sent via the free **Batsign** service, and by invocations of **external commands**. Each of these backends can be enabled or disabled independently of the others, and each has its own set of customizable strings.

### slack

Messages to Slack channels can trivially be pushed through use of [webhook URLs](https://en.wikipedia.org/wiki/Webhook). HTTP requests made to these will end up as messages in the channels they refer to. See [this guide](https://docs.slack.dev/messaging/sending-messages-using-incoming-webhooks) in the Slack documentation for developers on how to get started.

It is recommended that you make an entry in `/etc/hosts` to manually resolve `hooks.slack.com` to *an* IP of the underlying Slack server, to avoid potential DNS lookup failures.

URLs must be quoted. You may enter any number of URLs as long as you separate the individual strings with a comma.

```toml
[slack]
enabled = true
urls = ["https://hooks.slack.com/services/REDACTED/SECRET/KEY", "https://hooks.slack.com/services/SUPPORTS/MORE/THANONE"]
show_response = false
```

`show_response` will make the response body of the HTTP request be printed to the terminal.

#### formatting messages

Slack supports some formatting. Text between asterisks `*` will be in \***bold**\*, text between underscores `_` will be in \_*italics*\_, text between tildes `~` will be in \~~~strikethrough~~\~, etc.

Strings defined in the configuration file can make use of this.

```toml
[slack.strings]
alert_header = ":x: *PellX burner failure*"
reminder_header = ":alarm-clock: *PellX burner still in failure*"
startup_failed_header = ":x: *PellX burner startup _failed_*"
startup_success_header = ":fire: *PellX burner startup _succeeded_*"
```

See [this help article](https://slack.com/intl/en-gb/help/articles/360039953113-Format-your-messages-in-Slack-with-markup) for the full listing.

### batsign

[**Batsign**](https://batsign.me) is a free (gratis) service with which you can send brief emails. Requires registration, after which you will receive a unique URL that should be kept secret. HTTP requests made to this URL will send an email to the address you specified when registering.

It is recommended that you make an entry in `/etc/hosts` to manually resolve `batsign.me` to the IP of the underlying Batsign server, to avoid potential DNS lookup failures.

URLs must be quoted. You may enter any number of URLs as long as you separate the individual strings with a comma.

```toml
[batsign]
enabled = true
urls = ["https://batsign.me/at/name@host.tld/secretkey", "https://batsign.me/at/other@host.ld/supportsmultiple"]
show_response = false
```

`show_response` will make the response body of the HTTP request be printed to the terminal.

#### formatting mails

It is not possible to format text in Batsign emails with HTML markup. The best you can do is to get creative with Unicode characters, like [emoji](https://emojidb.org).

```toml
[batsign.strings]
alert_header = "❌ PellX burner failure"
reminder_header = "⏰ PellX burner still in failure"
startup_failed_header = "❌ PellX burner startup failed"
startup_success_header = "🔥 PellX burner startup succeeded"
```

### external commands

You can have the program execute external commands as a way to push notifications, although there are several caveats.

- The commands run will be passed several arguments in a specific hardcoded order, and it is unlikely that this will immediately suit whatever notification program you want to use. Realistically what you will end up doing is writing some glue-layer scripts that map the arguments to something the notifying programs can use. (Remember to `chmod` the scripts executable `+x`.)

- If you run the project binary as root, the external commands executed will in turn also be run as root. If you need them to be run as a different user, you will have to wrap them or have them recurse into themselves with something like `systemd-run` or `su`.

Command paths must be quoted. You may enter any number of commands as long as you separate the individual strings with a comma.

```toml
[command]
enabled = true
commands = ["/absolute/path/to/script.sh", "/absolute/path/to/other/script.sh"]
show_response = false
```

`show_response` will print the standard output and standard error of the command to the terminal.

#### arguments

The command-line arguments passed are as follows:

1. `$1`: The composed message body, formatted with strings as defined in the configuration file
2. `$2`: A string of the type of message, which is one of `alert`, `reminder`, `startup_failed` or `startup_success`
3. `$3`: The number of times the main loop has run, starting at 0
4. `$4`: The UNIX timestamp of when `LOW` was last read from the pellets burner, which qualifies as a desired state
5. `$5`: The UNIX timestamp of when `HIGH` was last read from the pellets burner, which qualifies as an error state
6. `$6`: The UNIX timestamp of when the reading from the pellets burner last *changed* (regardless of the values it went from or to)
7. `$7`: The UNIX timestamp of when the pellets burner last tried to start up, which is the first `LOW` after a `HIGH`

In cases where there is no UNIX timestamp to provide, the value passed will instead be `0`.

### `println`

The additional `println` backend is intended for logging and debugging purposes. It will print the composed message body to the terminal.

```toml
[println]
enabled = true
```

## systemd

The program lends itself to being run as a [**systemd**](https://systemd.io) service. This allows it to be automatically started on boot, and be restarted in the case of a crash.

To facilitate this, a basic service unit file is included in the repository. Copy it into `/etc/systemd/system/`, then use `systemctl edit` to modify it to point the `ExecStart` directive to the actual location of your compiled binary.

> If yours is in the default path of `/usr/local/bin/pellxd`, you can skip ahead to [**enable now**](#enable-now).

```sh
sudo cp pellxd.service /etc/systemd/system/
sudo systemctl edit pellxd.service
```

```ini
### Editing /etc/systemd/system/pellxd.service.d/override.conf
### Anything between here and the comment below will become the contents of the drop-in file

[Service]
ExecStart=
ExecStart=/home/user/src/pellxd/target/release/pellxd --verbose --config=/home/user/.config/pellxd.toml

### Edits below this comment will be discarded
### [...]
```

Be sure to include the empty `ExecStart=` line to clear the default value, as `Exec` directives are additive.

Do not include the `--debug` flag in the systemd service `ExecStart`, as it will make the program produce a very large amount of output.

### enable now

```sh
sudo systemctl enable --now pellxd.service
```

The `enable` command will make the service automatically start on boot, and the `--now` option will make it start immediately.

### logs

If the program is run via systemd, the output of the program can be found in [**the journal**](https://wiki.archlinux.org/title/Systemd/Journal).

```sh
journalctl -u pellxd.service
```

Logs will probably not persist across reboots. This is a common setting for Raspberry Pi OS to spare your SD card the extra writes. If logs spanning multiple boots are desirable, you will have to [**enable persistent logging**](https://www.google.com/search?q=systemd+how+to+enable+persistent+logging).

## ai

[**GitHub Copilot AI**](https://github.com/features/copilot/ai-code-editor) was used (in [**Visual Studio Code**](https://code.visualstudio.com)) for inline suggestions and to tab-complete some code and documentation. [**Claude**](https://claude.ai) was used to answer questions and teach Rust. No code from "write me a function doing *xyz*" prompts is included in this project.

## todo

- flesh out documentation (*correct* documentation)

## license

This project is licensed under the terms of the GNU General Public License version 2.0 or later (`GPL-2.0-or-later`). See the [LICENSE](LICENSE) file for details.
