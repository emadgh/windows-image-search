# People management correction contract

Automatic People clustering is derived state. User corrections are persistent state and must not be erased by ordinary reclustering.

## Assignment semantics

There are two different manual assignment modes:

- **Cluster anchor** — used when an automatic Person is named/materialized or when groups are merged. Anchored faces explicitly belong to the manual Person and allow newly discovered, unoverridden faces in the same compatible automatic cluster to inherit that manual identity.
- **Explicit face assignment** — used for split/move corrections and representative selection. It changes only that face and must never claim the rest of its automatic cluster.

Existing override databases are migrated with `propagates_cluster = 0`, so older assignments remain conservative rather than unexpectedly claiming a whole cluster.

## Corrections

- Rename/materialize: convert the current automatic group into a durable manual Person and anchor its current effective members.
- Merge: materialize each selected effective Person, merge their manual identities transactionally, and keep their anchored memberships.
- Split to new Person: explicitly assign only the selected face to a new manual Person.
- Move face: explicitly assign only the selected face to an existing manual Person.
- Detach: keep the face searchable but outside any effective Person group until restored or reassigned.
- Ignore: mark the face as not useful for People grouping.
- Restore automatic: delete that face override so the automatic snapshot becomes authoritative again.
- Representative: explicitly assign the selected face to the manual Person, then persist it as the representative.
- Delete manual Person: remove that manual identity and its assigned overrides; automatic clustering becomes visible again where applicable.

## Effective catalog resolution

Manual face dispositions always win over automatic membership. Cluster propagation is considered only from assignments explicitly marked as anchors. If one automatic cluster contains anchors for more than one manual identity, unoverridden faces are left with automatic membership rather than being forced into either manual Person.

This separation is required so future clustering/model changes can rebuild automatic state without silently losing names, merges, splits, ignored faces or representative choices.
