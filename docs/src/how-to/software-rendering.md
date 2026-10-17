# Run over ssh

archiveview draws with OpenGL. Two commands start it:

| command | draws with | use it |
| --- | --- | --- |
| `archiveview` | the machine's own OpenGL: the GPU, or whatever driver the desktop provides | normally: on a local desktop, a laptop, a Mac |
| `archiveview-ssh` | Mesa in software, on the CPU of the machine archiveview runs on | when the OpenGL path can't work, such as over `ssh -X` |

Both take the same arguments. Start with `archiveview`, and switch to
`archiveview-ssh` if it fails to start or draws nothing.

## When to use archiveview-ssh

- **Over `ssh -X` or `-Y`.** This is what it was named for. Forwarded OpenGL
  reaches your local X server as indirect GLX, which current X servers
  refuse, so `archiveview` exits with `glutin error: [2] BadValue`.
- **Where a GPU driver is installed but can't be used**, for example a
  remote desktop, VNC or Xvfb session on a machine with an NVIDIA driver:
  glvnd picks NVIDIA's GL, which then has no GPU display to draw on.
- **Where the GPU driver is broken** or misbehaves, as a workaround while
  you sort it out.

There is no need for it on macOS or on Wayland or X11 desktops with working
graphics. `--snapshot` needs neither: it draws without OpenGL or a display.

## What it does

`archiveview-ssh` is a small script that runs `archiveview` with:

- `LIBGL_ALWAYS_SOFTWARE=1`, so Mesa renders on the CPU (llvmpipe);
- `__GLX_VENDOR_LIBRARY_NAME=mesa`, and `__EGL_VENDOR_LIBRARY_FILENAMES`
  pointing at Mesa's EGL vendor file, so libglvnd picks Mesa rather than a
  GPU vendor's library such as NVIDIA's.

Over X forwarding the finished window then travels to your X server as
plain images, which every X server accepts.

Software rendering costs CPU on the machine running archiveview, and over
`ssh -X` each redraw sends the whole window over the network. archiveview
redraws only for your input, when new data arrives, and, while following
now, at the pace the time axis scrolls, so this is usually quick enough.

## Requirement

Mesa must be installed where archiveview runs: `mesa-dri-drivers` on RHEL,
`libgl1-mesa-dri` on Debian and Ubuntu (both are in the
[install prerequisites](install.md)).

## Example

```bash
ssh -X control-host
archiveview-ssh --archiver http://archiver:17668 SR-DI-DCCT-01:SIGNAL
```
