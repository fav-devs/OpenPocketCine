# USB Transfer Mode — Network Probe Result

## Summary

| Question | Answer |
|---|---|
| Link connected? | **No** |
| Picture appeared? | **No** |
| Camera screen | Showing Transfer File mode; SD card mounted as two drives |

## What happened

### RNDIS adapter
The camera enumerates an RNDIS (USB-Ethernet) interface as MI_00 (Class E0/01/03).
Windows assigned it **Ethernet 4**, but the camera did not serve DHCP — the PC fell
back to APIPA (`169.254.157.200/16`).

### IP address discovery
Scanned every `169.254.x.x` address via ARP/ping from the RNDIS socket — all
returned "Unreachable". The camera is **not** in the link-local range; it likely
uses a static address in a different subnet (probably `192.168.2.1`, the common
DJI USB address for Osmo-series cameras).

Assigning a matching static IP on the PC (`192.168.2.100/24`) requires administrator
elevation, which was not available during this session.

### TCP / UDP probes
UDP broadcast on `169.254.255.255:9004` and `:7001` — no reply.
TCP probes to `192.168.2.1:{7001,80,8080}` were skipped (not routable without the
static IP assignment).

### Viewfinder (`opc-monitor view --camera 192.168.2.1:9004`)
The binary launched and opened a Vulkan window (NVIDIA GeForce RTX 3060).
It printed:

```
drawing on NVIDIA GeForce RTX 3060
```

The window showed the **Finding** phase — no camera link formed, no picture.
After 30 seconds the process was terminated.

Because `192.168.2.1` is not reachable from `169.254.157.200` without a route,
the viewfinder could not reach the camera's UDP port at all.

### Camera screen (during Transfer File mode)
The Osmo Pocket 3 screen showed the file-transfer UI; the SD card appeared as two
drives in Windows Explorer. No camera output on the viewfinder.

## What's needed to complete the connection test

1. **Admin elevation** to assign `192.168.2.100/255.255.255.0` on Ethernet 4 (RNDIS).
2. Confirm the camera's actual IP (`arp -a` after step 1 will show it, or try
   `ping 192.168.2.1` from the static-IP'd adapter).
3. Re-run: `Apps\Desktop\target\debug\opc-monitor.exe view --camera 192.168.2.1:9004`

## Swift build note

`build-swift-core.bat` failed on the first attempt due to a Swift 6 keyword
regression: `internal` used as a parameter name/label needed backtick escaping
in `DesktopMediaABI.swift` and `MediaManifest.swift`. Fixed and re-ran
successfully before the viewfinder test.
