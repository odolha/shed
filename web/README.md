# Shed's web app

Shed in a browser, through a nexus (see
[Shed in a browser](../docs/user-guide.md#shed-in-a-browser) and
[Deploying a nexus](../docs/nexus-deploy.md)): the linked
workspaces with their benches, AI tabs, terminals, projects and Git, the
logbook, devices and notifications. Angular 22 (standalone components, signals,
zoneless), with a small UI kit of its own; no other UI library.

## Develop

Node 24.15 or newer (`.nvmrc`), or 22.22.3+.

```sh
npm install
npm run mock     # a pretend nexus on 127.0.0.1:8787 (web/mock)
npm start        # the app on http://localhost:4200, /api proxied to the mock
```

Sign in as `you` with the password `shed`; any six-digit code works but
`000000`. The mock has a workspace `devbox` (two benches: terminals with a tiny
fake shell, an AI tab with a scripted conversation, a sub-agent at work, Claude Code's notes and timers, files attached kept at a pretend path, a project tab with files and
Git; a closed bench) and an offline `laptop`. A workspace `buildbox` asks to
pair: approve it under Devices by typing its code `K7QF-2M9D`, or reject it
from its row. Its switches:

| Variable | Does |
|---|---|
| `PORT=8799` | another port (change `proxy.conf.json` to match) |
| `MOCK_FRESH=1` | no account yet: prints a `/setup#code=…` link for the first-run setup |
| `MOCK_VIEW=1` | `devbox` allows viewing only |
| `MOCK_LATENCY=300` | every answer comes that many ms late |
| `MOCK_NO_REVIEW=1` | the AI review answers `unsupported` |
| `MOCK_NTFY_TOPIC=https://ntfy.example.com/alerts-shed` | the nexus has a ntfy topic for phones (`PushInfo.ntfyTopic`) |

Notifications follow the nexus's rule: an AI tab that starts waiting or
finishes a turn is sent live (a toast) while the page is in view; otherwise the
mock logs what it would push. Typing `notify` in the mock's terminal sends a
test one.

To use a real nexus instead, point `proxy.conf.json` at it (HTTPS: `"secure": true`).

## Build and test

```sh
npm run build        # production build into web/dist/shed-web
npm test             # unit tests (Vitest, through `ng test`)
npm run check:mock   # type-checks the mock against the generated API types
npm run icons        # after changing assets/icons/ui/*.svg: remakes src/ui/icons.ts
```

`dist/shed-web` holds `index.html`, the hashed bundles, `sw.js`, the manifest
and icons. Release builds of `shed-nexus` build the app themselves (in cargo's
build folder, with the Node on `PATH`) and carry these files in the binary;
`web_dir` in `nexus.toml` serves a `dist/shed-web` from disk instead. Either
way the nexus falls back to `index.html` for the app's routes (`/w/…`,
`/devices`, `/settings`, `/login`, `/setup`), caches the hashed bundles for a
year and revalidates the rest. See
[The web app in the nexus](../docs/development.md#the-web-app-in-the-nexus).

## Layout

| Path | What |
|---|---|
| `src/api/gen/api.ts` | The API types, generated from `crates/shed-api` (`cargo run -p shed-codegen`); never edited |
| `src/api/` | `NexusHttp` (HTTP routes, cookie session, `X-Shed-Csrf`), `NexusSocket` (the `/api` WebSocket: requests by id, subscriptions, reconnect), `WorkspacesStore` (signals), `AuthService` and the route guards |
| `src/ui/` | The UI kit: icons, menus (popover layer), dialogs (`<dialog>`), tooltips, notices, usage limit bars, time formatting |
| `src/i18n/` | `en.json` and `t()` / the `t` pipe |
| `src/app/workspaces/` | The overview: workspaces and their benches |
| `src/app/bench/` | A bench's page: tab bar, new tabs, folder picker, bench actions |
| `src/app/ai/` | AI tabs: conversation, Markdown, permissions and questions, prompt, sessions, review |
| `src/app/terminal/` | Terminals from the screen's text: re-wrapping, keys, key bar |
| `src/app/logbook/`, `project/`, `git/` | The logbook; project tabs; Git |
| `src/app/setup/`, `login/`, `devices/`, `settings/` | The account, devices and pairing, push |
| `public/` | Service worker (push, notification clicks), manifest, icons |
| `mock/` | The pretend nexus |

## Rules

- Signals for state, `OnPush` everywhere, the built-in control flow, no
  NgModules.
- Every string goes through `t()`; keys are flat and dotted by feature.
- No `innerHTML` except Markdown, and that only through DOMPurify
  (`src/app/ai/markdown.ts`). No inline scripts or styles in `index.html`, so a
  strict Content-Security-Policy works: `default-src 'self'; img-src 'self' data:;
  style-src 'self' 'unsafe-inline'; connect-src 'self'` (Angular sets styles
  on elements, hence `unsafe-inline` for styles only).
- A change that is visible updates `docs/user-guide.md`; a change in the
  API is made in `crates/shed-api` and regenerated, never here.
