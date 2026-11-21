/**
 * What the product is called to a person: page titles, UI text, the manifest, the wordmark. The
 * next rename is this one line (plus the backend's `product_name!` in `backend/src/config.rs`).
 * Identifiers that installs depend on (cookie and storage keys, routes, `EUNOMIA_*` settings, the
 * `eunomia` MCP server key, paths, image names) do not use it and stay as they are.
 */
export const PRODUCT_NAME = "Cupel";

/** The lower-case wordmark shown beside the mark. */
export const PRODUCT_WORDMARK = PRODUCT_NAME.toLowerCase();
