# Zed Custom releases

The fork's default branch contains the changes to apply to each upstream Zed
nightly. The [Zed Custom nightly workflow](.github/workflows/fork_nightly.yml)
runs daily and can also be started manually. It takes the current upstream
`nightly` tag, applies the difference between the fork's `main` and its
upstream base, then builds macOS arm64 and Linux x86_64. It publishes a GitHub
Release only after both bundles pass their checks. The release includes the
matching remote server binaries.

Keep custom changes on the fork's `main` branch once they are ready to ship.
The workflow reads committed changes on GitHub; local uncommitted files are not
part of a release. Use squash merges for custom changes so the fork remains easy
to compare with upstream. If the patch conflicts with a new nightly, the run
fails and the previous release remains the latest. Resolve the conflict in the
fork and rerun the workflow. There is no automatic agent conflict resolution in
the workflow.

GitHub disables scheduled Actions in new forks by default. Enable Actions and
the workflow once after merging it to the default branch. The repository and
its releases must remain public for installed applications to read release
metadata and download assets without an embedded GitHub credential.

## First installation

Download `Zed-aarch64.dmg` or `zed-linux-x86_64.tar.gz` from the latest
Zed Custom release in this fork.

On macOS, install `Zed Custom.app` from the DMG. The app is signed ad hoc
without a Developer ID and is not notarized. Gatekeeper may block its first
launch. After attempting to open it, use **System Settings → Privacy & Security
→ Open Anyway** if you trust the build. The initial approval is a macOS action;
the application updater handles later releases. If macOS asks for approval
again after an update, that is a limitation of the unsigned distribution and
must be evaluated on the actual machine.

On Linux, from a checkout of this fork, install the downloaded archive with:

```sh
ZED_CHANNEL=nightly ZED_BUNDLE_PATH=/absolute/path/zed-linux-x86_64.tar.gz ./script/install.sh
```

The application is installed under `~/.local/zed-custom.app`, with the
`zed-custom` command and
`io.github.alexeyvatolin.ZedCustom.desktop` entry. Its settings and data are
under `zedcustom` paths, separate from the official Zed installation. The
built-in updater requires `rsync` on Linux.

Both platforms check GitHub Releases hourly. A release tag must be the full
version embedded in the built application, prefixed with `v`; the workflow
creates this tag. Because GitHub's `releases/latest` endpoint excludes
prereleases, the workflow publishes these nightlies as regular GitHub Releases
with a Nightly name.

## Verify the update path

Install release A, publish release B, and use **Check for Updates** in Zed
Custom. Verify that B downloads and is running after restart. Repeat on both
platforms and verify that an SSH connection downloads a remote server from the
same release as the installed client. The workflow's bundle checks cannot
replace this installed application test.
