# Earlier trials, not final acceptance

The initial native edit reached revision 10, but the process died at device
startup. The previously qualified playback executable also exited with SIGKILL.
`previous-device-limits.log` observes RLIMIT_RTTIME change from unlimited to zero.
GDB identifies PipeWire's RT module changing that limit through ALSA device setup.
The desktop portal reported all-zero Realtime properties, while system RTKit
reported 200000 microseconds. Restarting only the user xdg-desktop-portal service
restored MaxRealtimePriority=20, MinNiceLevel=-15 and RTTimeUSecMax=200000.
`repaired-device-six.json` then records 617 actual callbacks and exact final sample
145308 at 44.1 kHz. No audio settings or PipeWire services were changed.

PipeWire's [RT module contract](https://docs.pipewire.org/page_module_rt.html) and
[1.6.8 implementation](https://github.com/PipeWire/pipewire/blob/1.6.8/src/modules/module-rt.c)
explain the realtime allowance and clamping to the portal-reported maximum.
The runtime still hosted device-library code inside the application process.
[Issue #30](https://github.com/michaelmonetized/editbay/issues/30) tracks isolated
sound devices so this kind of backend failure cannot terminate the editor.

The first layout put a verified export, all edit controls and the viewer in one
column, leaving the viewer too small. The final wide layout uses two columns;
compact windows can collapse edit controls.

An intermediate driver's `qualified` field did not verify the reopened viewer's
sequence identity. Its screenshot shows the source sequence after a click landed
during the source-panel collapse animation. This is not accepted recovered-cut
proof. The final driver waits for that animation and verifies the saved record's
sequence ID in viewing, playback, reopened and compact-window observations.
The retained JSON and screenshot record the earlier gap in the qualification.
