# Browser

A small macOS browser written in Rust. It uses the Mac's built-in WebKit engine, so the app does not bundle a separate browser engine.

## Build and run

You need an Apple Silicon Mac, Apple's Command Line Tools, and a current stable Rust toolchain with Cargo. The build uses the versions pinned in `Cargo.lock` and downloads Rust dependencies on the first run.

From this folder, run:

```sh
bash scripts/build-app.sh
open dist/Browser.app
```

The script creates `dist/Browser.app` and `dist/Browser-Apple-Silicon.zip`. It signs the local build ad hoc. To rebuild after editing the source, run the same script again and reopen the app.

## Using the browser

- Enter a web address or search in the sidebar field. `⌘L` focuses it; `Esc` puts the page address back.
- Open a tab with the **New tab** button or `⌘T`; the start page shows bookmark and frequently visited tiles with names and icons. The address field is ready for typing. Close a tab with `⌘W`, and reopen it with `⇧⌘T`.
- `⌘`-click or middle-click a link to open it in a background tab (add `⇧` to switch to it). Links that open a new window become tabs; sign-in pop-ups keep their own window.
- Drag tabs to reorder them. Middle-click a tab to close it; right-click for Reload, Duplicate, Copy Address, and Close Other Tabs / Tabs Below.
- Switch tabs with `⌘1` through `⌘9`, `Control-Tab`, or `⇧⌘[` / `⇧⌘]`.
- `⌘R` reloads, `⌘.` stops, `⌘[` / `⌘]` go back and forward (mouse side buttons and trackpad swipes work too).
- `⌘F` finds on the page, `⌘G` / `⇧⌘G` go to the next or previous match.
- `⌘+`, `⌘-`, and `⌘0` zoom each tab; the address field shows the zoom level, and clicking it resets.
- `⌘P` prints. `⌃⌘F` enters full screen.
- Pointing at a link shows its address at the bottom of the page.
- Open tabs are restored at the next launch. Only the selected tab loads right away; the others load when you open them, so a large session costs no extra memory.
- **Protection → Memory → Unload Inactive Tabs After** controls routine background-tab unloading. The default is 30 minutes; choose **Never**, a preset, or **Custom…** for any whole number of minutes from 1 to 60. The setting persists across launches. Unloaded tabs reload when opened. Tabs playing media or using the camera or microphone are kept; the selected tab is never unloaded by this timer.
- WebKit is configured for memory over speed: no spare page process is started ahead of time, and no old pages or processes are cached for going back, so Back reloads the page.
- Video and audio don't autoplay; click to play. Autoplaying video can cost hundreds of megabytes.
- **Protection → Memory → Lockdown Mode** (on by default) uses WebKit's Lockdown Mode: no JavaScript compiler, WebGL, or other complex web features. Heavy pages use roughly 60–100 MB less, but big web apps run slower. Turning it off reloads the current tab.
- **Protection → Memory → Hide Ads on Every Site** (off by default) adds EasyList's site-independent hiding rules. Network blocking and site-specific hiding work without it; it costs up to about 90 MB more on heavy news pages.
- The sidebar follows the system's light or dark appearance.

## Ad and tracker blocking

The browser converts standard Adblock Plus filter lists into WebKit content rules, so blocking happens inside WebKit before a request is sent, and matching page elements are hidden.

- **Lists:** EasyList (ads) and EasyPrivacy (trackers) are on by default; Fanboy's cookie-notice list can be turned on from the **Protection** menu. Lists download in the background on first launch and refresh every four days. A small built-in list protects pages until then.
- **Per site:** the switch on the **Protection** card turns blocking off or on for the current site. `⇧⌘B` pauses it everywhere.
- **Custom filters:** **Protection → Edit Custom Filters…** opens a text file for your own rules in Adblock Plus syntax; saving it applies them.
- **Supported syntax:** network filters (`||domain^`, wildcards, `|` anchors, `$third-party`, `$domain=`, resource types, `$popup`, `$document`, `$important`, `$match-case`), exceptions (`@@`, `$elemhide`, `$generichide`), and element hiding (`##`, `#@#`). Rules that WebKit cannot express (regular-expression filters, `$csp`, `$redirect`, `$removeparam`, scriptlets and procedural selectors such as `:has-text()`) are skipped.

Downloaded lists and settings are stored in `~/Library/Application Support/Browser`. Compiled rules are cached by WebKit, so later launches start protected at once.

Cookies, site storage, and cache persist between launches. Downloads go to the Mac's Downloads folder. Closing the last tab leaves an empty window with a **New tab** button.

## Current limits

- The memory figure in the sidebar counts the app together with the WebKit processes that load its pages (hover it for the split). When macOS reports memory pressure, background tabs that aren't playing media can be unloaded even if routine unloading is set to **Never**.
- Passkeys for arbitrary websites require Apple's approved browser passkey entitlement and a properly provisioned signing identity. The ordinary ad hoc build does not have that entitlement. `macos/Browser.entitlements` and the optional `BROWSER_SIGNING_IDENTITY` build setting are prepared for a developer account that has Apple's approval.
- Some websites may restrict sign-in inside WebKit-based browsers independently of this app.
- WebKit does not report blocked requests to the app, so the browser cannot show a per-page blocked count. Video ads served from the same domain as the content (for example on YouTube) need script injection, which this blocker does not do.

Generated build files live in `target/` and `dist/`; neither is part of the source archive.
