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

- Enter a web address or search in the sidebar field.
- Open a tab with the **New tab** button or `⌘T`; close one with `⌘W`.
- Use `⌘L` for the address field, `⌘R` to reload, and `⌘[` / `⌘]` to navigate history.
- Switch tabs with `⌘1` through `⌘9` or `Control-Tab`.
- The **Privacy protection** control toggles the built-in ad and tracker rules.

Cookies, site storage, and cache persist between launches. Downloads go to the Mac's Downloads folder. Closing the last tab leaves an empty window with a **New tab** button.

## Current limits

- The memory figure in the sidebar shows the app process; macOS WebKit helper processes are separate.
- Passkeys for arbitrary websites require Apple's approved browser passkey entitlement and a properly provisioned signing identity. The ordinary ad hoc build does not have that entitlement. `macos/Browser.entitlements` and the optional `BROWSER_SIGNING_IDENTITY` build setting are prepared for a developer account that has Apple's approval.
- Some websites may restrict sign-in inside WebKit-based browsers independently of this app.

Generated build files live in `target/` and `dist/`; neither is part of the source archive.
