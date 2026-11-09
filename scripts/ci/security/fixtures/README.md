# Negative scanner fixtures

`vulnerable` locks lodash 4.17.15. It is never installed or used by Eunomia.
`secret` contains a fake GitHub token, generated for this test. It grants no access.
The repository scan excludes this directory only. `security-test.py` scans these
fixtures separately and checks that a rejected secret never appears in output.
