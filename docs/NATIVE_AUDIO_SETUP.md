# Native audio preparation

The ARM64 Omarchy speaker route under qualification includes PipeWire and an Asahi
speaker filter running inside WirePlumber. Both processes must remain resident
under memory pressure. A driver underrun was observed in the same second as a
PipeWire major page fault, with the editor's source ring still full. Scheduling
priority alone did not prevent that failure.

The resident speaker route still reported an underrun after 382.991 seconds,
without additional service page faults and with drift below 1.6 ms. Adding 4,096
hardware frames still underruns after 939.304 seconds with a full source ring.
The supplied Asahi speaker rule retains that headroom and selects interrupt
timing with a 2,048-frame hardware period. The protected speaker filter and
EditBay's 16,384-frame source ring remain in use. Backend timestamps retain the
added hardware latency. The recorded two-hour runtime completes with zero driver
underruns or dequeue misses, exact source end and reaped children. This qualifies
the measured native route; physical speaker/display timing remains separate.

EditBay requires PipeWire 1.4 or later for its realtime adaptive resampler. The
Asahi driver reports a small rate difference from the monotonic clock. The native
callback applies that measured ratio through `pw_stream_set_rate`, keeping source
playback aligned with both host and backend timestamps. Conversion stays inside
PipeWire; the Rust source ring retains its capacity and exact content end. Driver
underrun counters and both 20 ms clock checks remain active. The 180-second pilot
passes with maximum backend drift 0.087770 ms and host drift 0.112691 ms. The full
two-hour run passes the unchanged runtime gates at 0.194580 ms backend drift and
0.194913 ms host drift, with 145,344 KiB peak sampled combined RSS.

That frozen recorder retried four disk-write errors. Its strict zero-retry check
fails; independent audit verifies all 70,869 records, declared byte count, clock
and memory summaries, source/project hashes and a matching second read of the
stored trace. The current collector fails immediately on a recording error.
See [release evidence](evidence/shipping-candidate/README.md) for this distinction
and earlier failures.

On 2026-10-07, an installed cold-start trial exposed the running worker's 500 ms
response deadline being applied to process loading and the initial document
binding. The first request now has a separate five-second startup deadline;
every following request retains 500 ms. Cancellation still interrupts the wait,
and device callbacks, source progress, drift and retirement keep their existing
limits. Preparation requires granted realtime scheduling on the qualified host;
denial remains a visible startup failure.

The supplied configuration enables PipeWire's memory locking and limits each
service to 1 GiB of locked address space. The speaker filter needs more than
256 MiB of address space during startup. Locked address space includes reserved
mappings; the actual resident memory is recorded separately in qualification.
This setup retains the existing device profiles, speaker protection and route.

From the source checkout, install these fragments:

```sh
audio_uid=$(id -u)
audio_backup_suffix=".editbay-before-$(date -u +%Y%m%dT%H%M%SZ)"
manager_pid=$(systemctl show "user@$audio_uid.service" -p MainPID --value)
mkdir -p ~/.config/pipewire/pipewire.conf.d
mkdir -p ~/.config/wireplumber/wireplumber.conf.d
mkdir -p ~/.config/systemd/user/pipewire.service.d
mkdir -p ~/.config/systemd/user/wireplumber.service.d
install -b -S "$audio_backup_suffix" -m 644 assets/audio/residency.conf ~/.config/pipewire/pipewire.conf.d/50-editbay-audio-residency.conf
install -b -S "$audio_backup_suffix" -m 644 assets/audio/residency.conf ~/.config/wireplumber/wireplumber.conf.d/50-editbay-audio-residency.conf
install -b -S "$audio_backup_suffix" -m 644 assets/audio/asahi-speaker-headroom.conf ~/.config/wireplumber/wireplumber.conf.d/50-editbay-asahi-speaker-headroom.conf
install -b -S "$audio_backup_suffix" -m 644 assets/audio/memlock-service.conf ~/.config/systemd/user/pipewire.service.d/50-editbay-audio-memory.conf
install -b -S "$audio_backup_suffix" -m 644 assets/audio/memlock-service.conf ~/.config/systemd/user/wireplumber.service.d/50-editbay-audio-memory.conf
sudo install -D -b -S "$audio_backup_suffix" -m 644 assets/audio/memlock-service.conf "/etc/systemd/system/user@$audio_uid.service.d/50-editbay-audio-memory.conf"
sudo prlimit --pid "$manager_pid" --memlock=1073741824:1073741824
sudo systemctl daemon-reload
systemctl --user daemon-reload
systemctl --user restart pipewire wireplumber pipewire-pulse
```

The manager limit takes effect for the current session through `prlimit`; its
service fragment preserves the same limit after login. Configuration presence
does not establish qualification. Before testing, inspect each service's real
`/proc/PID/status` (`VmLck`, `VmRSS`, `VmSwap`), its thread scheduling and the
actual speaker device. The commands retain any existing named fragments beside
their replacement with the timestamped backup suffix. Require zero swapped service memory, real-time data
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
[realtime stream rate control](https://docs.pipewire.org/group__pw__stream.html),
[ALSA hardware buffering](https://docs.pipewire.org/page_man_pipewire-props_7.html),
[WirePlumber configuration fragments](https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/conf_file.html).
