# Virtual Plugin Repository

This fixture contains deterministic local-only plugins used by automated tests.

- `virtual-content`: primary content plugin for the `virtual` platform.
- `virtual-content-backup`: fallback content plugin for the `virtual` platform.
- `virtual-catalog`: content plugin for the independent `catalog` platform.
- `virtual-build-risk`: lifecycle-script authorization fixture.

All content data is static. Content plugins declare no allowed network hosts and
do not access external services.
