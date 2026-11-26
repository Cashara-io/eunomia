variable "region" {
  type    = string
  default = "ap-southeast-2"
}

variable "domain" {
  description = "Public hostname of the hosted Cupel server. Its DNS A record must point at the Elastic IP."
  type        = string
  default     = "cupel.buildpaperless.com.au"
}

variable "acme_email" {
  description = "Let's Encrypt contact address for the certificate."
  type        = string
}

variable "instance_type" {
  description = "Graviton (arm64); the Cupel images are multi-arch."
  type        = string
  default     = "t4g.large"
}

variable "root_volume_gb" {
  type    = number
  default = 100
}

variable "cupel_version" {
  description = "Git tag of Cashara-io/eunomia to run; images use the same tag."
  type        = string
  default     = "v2.3.0"
}

variable "billing_url" {
  description = "Where an organisation owner goes to pay or change seats (EUNOMIA_BILLING_URL)."
  type        = string
}

variable "snapshot_retain_days" {
  type    = number
  default = 14
}

variable "backup_retain_days" {
  type    = number
  default = 90
}
