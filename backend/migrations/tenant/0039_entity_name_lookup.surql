-- Cross-kind name lookup needs a range on name_key without a kind equality.
DEFINE INDEX OVERWRITE entity_vault_name_key_idx ON entity FIELDS vault, name_key;
