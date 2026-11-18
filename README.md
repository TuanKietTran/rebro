# rebro

One place for your GitHub, GitLab, Codeberg/Forgejo and Gitea accounts, on public or self-hosted instances. You sign in once per forge, and after that git, `gh` and `glab` stay logged in.

rebro comes as a CLI, a terminal UI and a native desktop app. The desktop app is built with GPUI and follows the platform's own look on Windows, macOS and Linux (GNOME).

## Why

`gh auth login` creates an **OAuth-app token**. GitHub allows at most 10 of these per user per app, and when an 11th is created it silently revokes the oldest. Every login counts: another machine, a devbox, WSL, a container. Logging in again only restarts the countdown, so `gh` keeps getting logged out somewhere.

rebro owns the credential instead. It signs in with **its own** OAuth client, or uses a long-lived token you paste. It keeps the token in the OS credential store and acts as git's credential helper, so every tool reads from one place.

## Install

```bash
./install.sh               # macOS / Linux: ~/.local/bin/rebro (+ ~/Applications/rebro.app on macOS)
./install.sh --no-gui      # CLI only
```
```powershell
pwsh -File install.ps1     # Windows: %LOCALAPPDATA%\Programs\rebro, user PATH, Start Menu entry
```

Both scripts build a release binary, install it at a fixed path (so git's helper entry never moves) and run `rebro sync`.

Requirements:
- Rust (rustup).
- On macOS, the Xcode command line tools.
- On Windows, the GNU toolchain is enough. Release builds precompile GPUI's shaders with `fxc.exe` from the Windows SDK (`winget install --id Microsoft.WindowsSDK.10.0.26100 -e`). Without the SDK, `install.ps1` installs a dev-profile GUI that compiles its shaders at runtime from the cargo registry. That works, but rerun the script once the SDK is installed.

## Add your accounts

### Sign in (recommended)

| Forge | Flow | OAuth client |
|---|---|---|
| github.com | device code: the browser opens and the code is on your clipboard | pre-registered public client, separate from gh's |
| gitlab.com | device code | pre-registered public client (scopes `read_repository`/`write_repository`, so the account is named `user-<id>`) |
| codeberg.org, gitea.com | browser → approve → back to `127.0.0.1` (PKCE) | pre-registered |
| any Gitea ≥ 1.22 / Forgejo | same | the client every instance ships for git helpers |
| self-hosted GitLab, GitHub Enterprise | same | your own app: `rebro login --host … --client-id …` |

GitLab and Gitea access tokens expire after 1–2 hours. rebro keeps the refresh token in its own credential entry. Whenever git, `rebro check` or `rebro sync` needs an access token that is within 5 minutes of expiring, rebro renews it.

### Import what the computer already has

`rebro import` (TUI: `i`; app: **Import from this computer**) collects the tokens already on the machine:
- **gh**: every account on every host
- **glab** and **tea** configs
- **git's credential helpers**: Git Credential Manager, macOS Keychain, libsecret, cache and others. Each helper is asked non-interactively for the public forges, every host rebro already knows, and any `--host …` you pass
- `~/.git-credentials`
- `GH_TOKEN`, `GITHUB_TOKEN`, `GITLAB_TOKEN`, `GLAB_TOKEN`, `CODEBERG_TOKEN`

Each token is checked with its forge before it is kept. When several tokens belong to one account, rebro keeps the best one (fewest warnings, then no expiry). Accounts whose rebro token still works are left alone. Tokens are never printed. Add `--dry-run` to see what would happen, and `--json` for scripts.

An imported `gho_` token from gh is still a gh OAuth token, so the 10-token limit can still revoke it. `rebro check` keeps warning about it until you sign in with rebro or paste a personal access token.

### Paste a token

`rebro add --forge github --open` opens the forge's token page and reads the token at a hidden prompt. A classic GitHub personal access token can be set to never expire, and it is not an OAuth-app token, so the 10-token limit doesn't apply to it. rebro reads the token back from the credential store after writing it, to prove the store kept it.

## Use

```text
rebro                          terminal UI (same as `rebro tui`)
rebro gui                      desktop app
rebro login --forge github     browser sign-in
rebro add --forge codeberg     paste a token
rebro add --host git.example.com --forge gitea
rebro import [--dry-run]       take over what gh, glab, tea and git already have
rebro ls | check | sync | doctor   (--json for scripts)
rebro default github.com/work
rebro owners github.com/work acme other-org   # route those owners' repos to that account
rebro rm github.com/old
```

`rebro check` verifies every token against its forge (identity, scopes, expiry). It warns 14 days before a token expires, and whenever a token is a revocable gh OAuth token.

## How git and gh use it

When a host has several accounts, rebro routes each repo by its owner: first the owners listed with `rebro owners`, then the account's own username, then the host's default account. This is why `sync` turns on `credential.<host>.useHttpPath`.

`rebro sync` changes the following:
- **`~/.gitconfig`, for each host:** first an empty `credential.https://<host>.helper` line, which clears Git Credential Manager and gh's helper for that host, then rebro's own `git-credential` helper. The previous values are backed up to `git-helper-backup.txt` in the config directory.
- **gh and glab:** `gh auth login --with-token` and `glab auth login --stdin` for each host's default account. The token goes in on stdin, never on the command line.
- **git's `store`/`erase` requests are ignored**, so a failed push can't delete a token rebro manages. Fix tokens in rebro instead.

## Where things live

| What | Where |
|---|---|
| `accounts.toml`: forge, host, username, default, owners, sign-in kind. **No secrets.** | config dir: `%APPDATA%\rebro`, `~/Library/Application Support/rebro`, `~/.config/rebro` |
| `local.toml`: this machine's last checks and token expiry | next to `accounts.toml` |
| tokens | Windows Credential Manager (`<host>/<user>.rebro`), macOS login Keychain (service `rebro`), Secret Service on Linux |

Apps launched from Finder on macOS get a bare `PATH`, so rebro adds the usual Homebrew, MacPorts, Nix, `~/.local/bin` and `~/.cargo/bin` directories itself in order to find gh and glab.

## Several machines

`accounts.toml` holds no secrets, so you can sync it between your machines with any file-sync tool. Tokens never leave a machine's own credential store.
- **Accounts added elsewhere:** an account added on another machine shows up as **"not signed in on this machine yet"**. Sign in once on this machine; after that, each machine refreshes its own token.
- **What checks write:** checks write only `local.toml`, so routine use never touches the shared file, and identical content is never rewritten.
- **Conflicting edits:** if two machines change the list while out of sync, Syncthing-style conflict copies (`accounts.sync-conflict-*.toml`) are merged in on the next start. The merge is a union by account, so nothing is dropped, and the copy is deleted afterwards. An account removed on one side can come back this way; remove it again.

## Design

The UI is designed first in Penpot (file **rebro**), then implemented to match:
- **Windows (Fluent):** Settings-style navigation, cards, ContentDialogs, a MenuFlyout and InfoBars.
- **macOS:** a sidebar, a unified toolbar, an inset grouped list and sheets.
- **GNOME (libadwaita):** a split view, boxed lists and AdwDialog.

The app picks the look from the OS and follows the system's light/dark setting live. To preview the other looks on one machine, set `REBRO_STYLE=windows|mac|gnome` and `REBRO_THEME=light|dark`. Design exports sit next to screenshots of the running app in [docs/design](docs/design/README.md).

## Development

```bash
cargo test --workspace                      # model, routing, credential protocol, TOML, import parsing
bash tests/e2e-credential.sh target/debug/rebro   # macOS Keychain / Linux Secret Service
pwsh -File tests\e2e-credential.ps1         # Windows Credential Manager
```

The end-to-end tests use real git and the real credential store, fully isolated: their own config directory (`REBRO_HOME`), their own credential service (`REBRO_KEYRING_SERVICE`) and a temporary git config, with gh and glab untouched. CI (`.github/workflows/macos.yml`) runs the unit tests, the macOS end-to-end test and `install.sh` on Apple Silicon, then starts the app.
