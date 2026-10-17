# Install archiveview

archiveview runs on x86_64 and aarch64 Linux and on macOS 11 or later
(Apple silicon or Intel). `install.sh` picks the release build to match the
machine.

## Install the prerequisites

On Debian and Ubuntu:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
```

On RHEL, Rocky, Alma and Fedora:

```bash
sudo dnf install libxkbcommon libwayland-client libglvnd-egl mesa-dri-drivers
```

None are needed on macOS.

## Install the release

```bash
curl -sSfL https://raw.githubusercontent.com/gilesknap/archiveview/main/install.sh | bash
```

This installs into `~/.local` (set `PREFIX` to choose another place):

| file | what |
| --- | --- |
| `bin/archiveview` | the plotter |
| `bin/archiveview-ssh` | runs the plotter with software OpenGL, for `ssh -X`; see [Run over ssh](software-rendering.md) |
| `bin/archiveview-mockarchiver` | a mock Archiver Appliance with synthetic history; see [Mock archiver](../reference/mockarchiver.md) |
| `share/archiveview/` | the README, licence and `THIRD_PARTY_LICENSES.txt` |

The Linux binaries are linked against glibc 2.28, so they run on RHEL 8+,
Ubuntu 20.04+ and Debian 10+.

Run from inside an unpacked release tarball, `install.sh` installs that
tarball rather than downloading one.

## Put it on your PATH

On macOS `~/.local/bin` is not on `PATH` by default; add this to `~/.zshrc`:

```bash
export PATH=$HOME/.local/bin:$PATH
```

## Upgrade or remove

Run the install command again to upgrade. To remove archiveview, delete the
`bin/archiveview*` files and `share/archiveview` from `~/.local`. Your saved
views and settings are in `~/.config/archiveview`.
