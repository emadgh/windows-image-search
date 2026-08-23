# Collections-first library/indexing

The user-facing Library / Indexing concept is removed. Collections are the only place where folders are added, removed, rescanned, or configured for face detection.

## Existing databases

On first load, any registered portable roots that are not already covered by a Collection are assigned to a virtual `Imported Library` Collection. This migration does not move source files and does not delete or rewrite `.imagesearch` data.

## Adding folders

Adding or dropping a directory into a Collection attaches/initializes that directory as a portable indexed root when it is not already covered by a registered root. The Collection membership is then saved and a normal changed-file rescan is scheduled for newly attached roots.

Individual file assignments remain for already indexed images. New content should be introduced by adding its containing folder so the indexing scope remains explicit.

## Removing content

Deleting a Collection or removing its folder/file assignments never deletes source images or `.imagesearch` directories. Session root registrations with no remaining Collection reference are detached from the active library and can be reattached later by adding that folder to a Collection again.

## UI

Rescan changed, Force rescan all, pause/resume, indexing progress, face-detection eligibility, folder membership, and manual file membership all live in the Collections Preferences category. The layout is vertical/full-width to remain usable in a resizable Preferences window.
