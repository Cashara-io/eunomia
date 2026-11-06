-- Pocket audio (#117), opt-in per connection (`config.store_audio`): the bytes live in the org's
-- `documents` bucket at `audio_path`; NONE = not stored (off, not yet uploaded by the device, over the
-- size cap, or no bucket configured).
DEFINE FIELD OVERWRITE audio_path ON pocket_recording TYPE option<string>;
DEFINE FIELD OVERWRITE audio_media_type ON pocket_recording TYPE option<string>;
DEFINE FIELD OVERWRITE audio_bytes ON pocket_recording TYPE option<int>;
