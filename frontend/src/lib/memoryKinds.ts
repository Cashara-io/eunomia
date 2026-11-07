// What a memory point is, in words: memory types from the backend ("world" is a plain fact), or an
// item from a connected source.
const POINT_KIND: Record<string, string> = { world: "Fact", experience: "Experience", observation: "Summary", record: "Saved item" };

export const pointKindLabel = (kind: string) => POINT_KIND[kind] ?? kind.charAt(0).toUpperCase() + kind.slice(1);
