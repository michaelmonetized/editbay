# Shared local brand banks

EditBay uses the existing client `.omabrand` directory as shared context. It does
not modify Omadesign services or change a bank's existing `brand.json` schema.
New directory banks write Omadesign's version-1 name manifest only when absent.
Legacy marker files remain intact and cannot receive directory-bank writes.

EditBay details live in `editbay.v1.json`: schema, monotonic revision, client,
project, notes, palettes and imported asset receipts. Unknown or corrupt schemas
stay visible and preserve their source bytes. A future migration must produce an
explicit conversion report before replacing either application's data.

| Kind | Shared storage | Accepted local files |
| --- | --- | --- |
| Fonts | `fonts/` | TTF, OTF, TTC |
| Logos | `logos/` | SVG and supported raster extensions |
| Artwork | `artwork/` | SVG, raster and native `.oma` documents |
| Title packages | `editbay-titles/` | `.oma`, `.editbay`, `.editbay-title`, ZIP |
| LUTs | `luts/` | CUBE, 3DL, SPI1D, SPI3D, CLF, CTF |

The R1 bank stores and exports original bytes. Font application, title rendering,
LUT execution and media preview belong to their authoring/render milestones.
Native asset packages are not executed by the bank.

Import verifies a regular source's identity, length and timestamps while copying
and records its complete SHA-256. Existing names are collisions, including an
identical file: choose a different name or retain the existing asset. New assets
and exports use synchronized, separate-file publication and never overwrite a
destination. Export checks the selected asset's expected hash again; changed
bytes require refresh and an explicit new selection.

Metadata writes hold a persistent bank lock and require the exact manifest hash
and directory identity originally loaded. A competing writer or replaced folder
is a visible conflict. Refresh reloads ownership. Existing Omadesign manifest
bytes remain unchanged, including unknown future versions.
The native pane retains locally edited fields across asset imports and refresh,
while accepting updated receipts and fields the user did not edit. Edits made
after a save starts remain unsaved after its acknowledgement. Bank selection and
normal application close require saving or explicitly discarding the draft.

Asset publication and receipt publication are separate durable operations. If
the second operation fails, EditBay reports the retained asset path and asks the
user to refresh and inspect it. It does not acknowledge a completed import or
delete a published asset. A cancelled scan/copy stops underlying chunked work;
publication already in progress must finish before cancellation is acknowledged.

Scans exclude hidden paths and symlinks. They process at most 10,000 entries and
2,048 assets, with 256 MiB per asset and bounded metadata reads. Incomplete scans,
changed or missing receipt files, unreadable entries and unsupported manifests
remain visible. Cancellation and file work run on the bank worker.
