# Ignored Plan Items

## Phase 3: CachingProvider (DuckDB transparent cache layer)
- **Plan source**: 03-caching-provider.md, 07-implementation-roadmap.md Phase 3
- **Status**: Not applicable
- **Reason**: The `midas-store` crate does not exist in this workspace. CachingProvider requires `DbHandle` from `midas-store`, which has not been created yet. The entire Phase 3 (CachingProvider, DuckDB integration) is deferred until `midas-store` is built. The architecture is designed to work without caching -- `TestProvider` is registered directly in the `ProviderRegistry`, and when `midas-store` is added later, it wraps `TestProvider` in `CachingProvider` transparently.
