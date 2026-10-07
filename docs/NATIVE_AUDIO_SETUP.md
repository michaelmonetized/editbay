# Native audio preparation

The qualified ARM64 Omarchy speaker route includes PipeWire and an Asahi
speaker filter running inside WirePlumber. Both processes must remain resident
under memory pressure. A driver underrun was observed in the same second as a
PipeWire major page fault, with the editor's source ring still full. Scheduling
priority alone did not prevent that failure.

The supplied configuration enables PipeWire's memory locking and limits each
service to 1 GiB of locked address space. The speaker filter needs more than
256 MiB of address space during startup. Locked address space includes reserved
mappings; the actual resident memory is recorded separately in qualification.
This setup retains the existing device profiles, speaker protection and route.

From the source checkout, install these fragments:

```sh
audio_uid=$(id -u)
manager_pid=$(systemctl show "user@$audio_uid.service" -p MainPID --value)
mkdir -p ~/.config/pipewire/pipewire.conf.d
mkdir -p ~/.config/wireplumber/wireplumber.conf.d
mkdir -p ~/.config/systemd/user/pipewire.service.d
mkdir -p ~/.config/systemd/user/wireplumber.service.d
install -m 644 assets/audio/residency.conf ~/.config/pipewire/pipewire.conf.d/50-editbay-audio-residency.conf
install -m 644 assets/audio/residency.conf ~/.config/wireplumber/wireplumber.conf.d/50-editbay-audio-residency.conf
install -m 644 assets/audio/memlock-service.conf ~/.config/systemd/user/pipewire.service.d/50-editbay-audio-memory.conf
install -m 644 assets/audio/memlock-service.conf ~/.config/systemd/user/wireplumber.service.d/50-editbay-audio-memory.conf
sudo install -D -m 644 assets/audio/memlock-service.conf "/etc/systemd/system/user@$audio_uid.service.d/50-editbay-audio-memory.conf"
sudo prlimit --pid "$manager_pid" --memlock=1073741824:1073741824
sudo systemctl daemon-reload
systemctl --user daemon-reload
systemctl --user restart pipewire wireplumber pipewire-pulse
```

The manager limit takes effect for the current session through `prlimit`; its
service fragment preserves the same limit after login. Configuration presence
does not establish qualification. Before testing, inspect each service's real
`/proc/PID/status` (`VmLck`, `VmRSS`, `VmSwap`), its thread scheduling and the
actual speaker device. Require zero swapped service memory, real-time data
threads and the intended hardware route. Retain the previous fragments if a
machine already has these names.

EditBay rejects changes in the driver's accumulated underrun duration even when
the one-cycle recovery flag is no longer present. The two-hour test retains both
those counters and the unmodified 20 ms backend/host drift gate. Confirm the
hardware separately: playback through Dummy Output cannot establish speaker
device qualification.

The package includes these fragments under `share/editbay/audio`. They are host
configuration data; installing an application version does not restart audio
services.

Sources: [PipeWire memory properties](https://docs.pipewire.org/page_man_pipewire_conf_5.html),
[driver clock observations](https://docs.pipewire.org/structspa__io__clock.html),
[WirePlumber configuration fragments](https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/conf_file.html).
