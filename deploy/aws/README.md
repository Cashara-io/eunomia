# Hosted Cupel on AWS

One server runs every hosted customer. A purchase on the Paperless website calls this server's
operator API, which creates an organisation with its own SurrealDB database
(see `docs/deployment.md`, "Running a hosted service"). Nothing here creates a server per customer.

Account `929214127672`, region `ap-southeast-2`. State: `s3://cupel-tfstate-929214127672/hosted/terraform.tfstate`
(versioned, encrypted, lock file). Every resource is tagged `Project=cupel`.

## What it builds

- VPC with one public subnet; security group with 80/443 in only. No SSH: use SSM Session Manager
  (`aws ssm start-session --target <instance_id>`).
- `t4g.large` Ubuntu 24.04 arm64, 100 GB encrypted gp3 root, IMDSv2 only (hop limit 1, so containers
  cannot reach instance credentials), termination protection on, Elastic IP.
- Daily EBS snapshots (DLM, 14 kept), and the nightly encrypted SurrealDB export copied to
  `s3://cupel-hosted-backups-929214127672/surreal/` at 04:00 Sydney (90 days).
- EC2 auto-recover on a failed system status check; CPU alarm; both email via SNS.

## Secrets

Not in Terraform state. They live in SSM Parameter Store under `/cupel/hosted/` (SecureString) and the
instance reads them at boot into `/opt/cupel/.env` (mode 600). A copy is in 1Password, Developers vault,
"Cupel hosted server (AWS)". Keys: `jwt_secret`, `encryption_key`, `surreal_pass`,
`backup_encryption_key`, `operator_token`, `openai_api_key`, `admin_email`, `smtp_*`.

**Losing `backup_encryption_key` makes every backup unreadable.** Losing `encryption_key` makes stored
connector credentials unreadable.

## Budget

AWS Budgets `cupel-monthly` (US$250, filtered on the `Project=cupel` cost allocation tag) and
`account-monthly-gross` (US$700, whole account, credits excluded). Alerts at 50/80/100% actual and
100% forecast. The `Project` tag must be activated as a cost allocation tag (Billing, Cost allocation
tags) once AWS has seen it, or the Cupel budget reads zero.

## Apply

```bash
aws login --region ap-southeast-2
tofu init && tofu plan && tofu apply
```

`user_data` and the AMI are ignored after creation, so an apply never replaces the server. To upgrade
Cupel, change the tag on the box (`/opt/cupel`, `EUNOMIA_IMAGE_TAG` in `.env`) and `docker compose up -d`.

## DNS

`cupel.buildpaperless.com.au` A record -> the `public_ip` output. Caddy gets the Let's Encrypt certificate
once the record resolves.

## Current state (23 Nov 2026)

The hosted operator API (#254, #256, #257) is on `main` but in no release yet, and images are only
published for `v*` tags. The server therefore runs `main` at `8a3409f3` with images built on the box
(`EUNOMIA_IMAGE_TAG=main-8a3409f3`, `docker compose build`). Once a release containing #257 is cut,
set `EUNOMIA_IMAGE_TAG` and the checkout to that tag and `docker compose pull && docker compose up -d`.
