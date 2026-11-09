-- Cover name pages and typed history traversal without loading a kind into the app.
DEFINE INDEX OVERWRITE entity_vault_kind_name_idx ON entity FIELDS vault, kind, name;
DEFINE INDEX OVERWRITE entity_property_num_page_idx ON entity_property FIELDS property, value_num, entity;
DEFINE INDEX OVERWRITE entity_property_text_page_idx ON entity_property FIELDS property, value_text, entity;
DEFINE INDEX OVERWRITE entity_property_bool_page_idx ON entity_property FIELDS property, value_bool, entity;
