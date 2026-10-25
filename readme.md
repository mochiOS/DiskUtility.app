# Disk Utility

Native AppCore / ViewKit disk, partition and mounted-volume inspection. The existing
960 × 640 layout, omochi controls, VK Symbols and application icon are preserved.
Inspection is read-only; Mount/Unmount, Partition and Erase remain disabled.

## mochiOS

The app obtains `appcore::storage::inventory()` on a worker thread. It requests
`fs.read.all` to open a read-only namespace-root Handle through POSIX, and queries
filesystem.service through that retained Handle. It never receives raw disk or
write authority. The provider rejects ordinary IPC, non-root objects, CONTROL,
attached Handles and malformed requests. No new kernel API is added.

The service reports mDriver disk capacity, GPT identities and partition extents,
plus live ext4 accounting for its actual Data and System mounts. Physical
inspection is bounded to 16 devices and 128 partitions per device. If the active
legacy disk session prevents mDriver inspection, the service reads validated GPT
metadata only on disks it already mounted. Those disk capacities are explicitly
labelled **GPT capacity**; they are not presented as hardware inquiry results.
GPT CRCs, ranges, overlap and identities are validated. Disk-space gaps are named
**Outside partitions** because they can include partition-table metadata and are
not a claim of usable, writable space.

Mounted volumes are linked to partitions only by the owner's exact device/range
mapping. Missing or ambiguous mappings leave mounted volumes as independent
entries. Unmounted filesystems are not opened or guessed from partition type.
Unknown access, connection type and filesystem remain unknown. Read-only state
and usage come from the mounted filesystem owner; filesystem capacity is distinct
from partition capacity. Mount metadata is registered by the owner during its
current startup mounting flow; dynamic mount operations are not implemented.

Provider snapshots are cached for two seconds and pages carry a generation.
AppCore rejects mixed generations, duplicate identities and invalid parent trees,
and retries interrupted pagination within a bound. Refresh preserves selection
and disclosure by identity. Errors retain prior data with a stale-data message;
partial physical discovery retains available disk and mounted-volume information.
A failed/empty query never becomes a sample or a zero-capacity disk.

## Linux preview

`mmake preview-disk-utility` opens the app; `mmake ui` includes it in the catalogue.
Linux continues to use explicitly labelled sample disks and never enumerates,
opens, mounts or modifies host disks. Set `MOCHIOS_PREVIEW_DISK_UTILITY_DEMO=0` to
check the unsupported-connection/error state. Preview and live results use the
same tree/detail components. Below 700px, list and details remain separate views
with a Disks back button.

## Verification

Tests cover the wire bounds and encoding, physical ranges and malformed tables,
root-Handle authorization and query-only dispatch using a disposable ext4 image,
real volume accounting, pagination generations, AppCore tree validation, stable
selection, stale data, missing attributes and existing compact/wide UI behavior.
Build verification covers Linux and the native filesystem.service / application.
Visual and on-device verification are performed by the user.

The icon remains the unchanged `docs/resources/appicons/Disk Utility.app.png` copy.
Partition/eject VK Symbols are still pending; existing symbols and text are used.
