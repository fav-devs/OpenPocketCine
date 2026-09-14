# Dumps everything Windows knows about a connected DJI camera's USB descriptors.
# Run once per USB mode the body offers (Webcam, Transfer File, anything else),
# naming the mode:  powershell -ExecutionPolicy Bypass -File tools\usb-descriptor-dump.ps1 webcam
# Output lands in usb-descriptors\<mode>-windows\; paste or commit the folder.
# For the full descriptor tree also install USB Device Tree Viewer
# (https://www.uwe-sieber.de/usbtreeview_e.html) — the script uses it when it is on PATH
# or in the current folder.
param([string]$Mode = "unknown")
$ErrorActionPreference = "Continue"
$vendor = "VID_2CA3"   # DJI
$out = "usb-descriptors\$Mode-windows"
New-Item -ItemType Directory -Force -Path $out | Out-Null
"mode: $Mode`nhost: $([System.Environment]::OSVersion.VersionString)`nwhen: $((Get-Date).ToUniversalTime().ToString('o'))" | Set-Content "$out\README.txt"

$devices = Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like "USB\$vendor*" }
if (-not $devices) {
  Write-Error "no DJI device ($vendor) on the bus — is the camera plugged in and in a USB mode?"
  exit 2
}

$lines = @()
foreach ($d in $devices) {
  $lines += "=== $($d.InstanceId)"
  $lines += "  name: $($d.FriendlyName)"
  $lines += "  class: $($d.Class)  status: $($d.Status)"
  foreach ($key in @('DEVPKEY_Device_BusReportedDeviceDesc','DEVPKEY_Device_HardwareIds','DEVPKEY_Device_CompatibleIds','DEVPKEY_Device_Service','DEVPKEY_Device_DriverDesc','DEVPKEY_Device_Parent','DEVPKEY_Device_Children','DEVPKEY_Device_LocationInfo')) {
    try {
      $p = Get-PnpDeviceProperty -InstanceId $d.InstanceId -KeyName $key -ErrorAction Stop
      $lines += "  $key = $(@($p.Data) -join ' | ')"
    } catch {}
  }
}
$lines | Set-Content "$out\pnp-devices.txt"

# Interfaces the composite parent split out (video, audio, vendor, storage...).
Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like "USB\$vendor*" -or $_.InstanceId -like "*$vendor*" } |
  Select-Object InstanceId, Class, FriendlyName, Status | Format-Table -AutoSize | Out-String -Width 300 |
  Set-Content "$out\pnp-table.txt"
pnputil /enum-devices /connected 2>$null | Out-String | Set-Content "$out\pnputil.txt"
Get-CimInstance Win32_PnPEntity | Where-Object { $_.PNPDeviceID -like "*$vendor*" } |
  Select-Object PNPDeviceID, Name, Service, Status | Format-List | Out-String -Width 300 |
  Set-Content "$out\win32-pnpentity.txt"

$tree = Get-Command UsbTreeView.exe -ErrorAction SilentlyContinue
if (-not $tree -and (Test-Path ".\UsbTreeView.exe")) { $tree = Get-Item ".\UsbTreeView.exe" }
if ($tree) {
  & $tree.Source /R "$out\usbtreeview.txt" | Out-Null
} else {
  "UsbTreeView.exe not found; only the PnP view was saved. Install USB Device Tree Viewer and run again for the raw descriptors." |
    Set-Content "$out\NOTE.txt"
}

Write-Host "wrote $out:"
Get-ChildItem $out | Select-Object -ExpandProperty Name
