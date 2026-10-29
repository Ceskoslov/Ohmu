# Recorded evidence

- [10,000-source measurement](scale-10000.json): optimized strict live and frozen queries, setup, distributions and disclosed hardware.
- [Original full-parse baseline](scale-10000-baseline.json): same generated corpus before ADR-010.
- [Historical producers](legacy.json): real archived CLI producers read/reindexed by the current reader; M2 backup/state round trip.

Results were captured from the implementation working tree based on the recorded
commit. `dirty:true` is intentional provenance, not a claim that a different
commit was tested. The generator and implementation are committed alongside these
reports. Generated input data, Wiki files, API credentials and model journals are
not committed. Reproduce using the scripts in [SCALE](../SCALE.md) and
[RELEASE](../RELEASE.md). Model quality and storage-cold results are absent.
