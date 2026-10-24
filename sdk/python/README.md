# eunomia (Python)

Client for the Eunomia REST API. No dependencies, Python 3.9 or newer.

```python
from eunomia import Eunomia

eu = Eunomia("http://localhost:3000", token="<personal API token>")
a = eu.create_vault("Research")
b = eu.create_vault("Notes")
eu.memory_write("Ada", "person", "Ada prefers tea", vault_id=a.id)
eu.memory_write("Ada", "person", "Ada lives in Perth", vault_id=b.id)
merged = eu.merge_vaults(a.id, b.id, name="Everything")
print(eu.recall("where does Ada live", vault_id=merged.id))
```

See [../README.md](../README.md) for the full method list.
