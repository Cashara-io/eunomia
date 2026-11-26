output "public_ip" {
  description = "Point the domain's DNS A record here."
  value       = aws_eip.cupel.public_ip
}
output "instance_id" { value = aws_instance.cupel.id }
output "backups_bucket" { value = aws_s3_bucket.backups.bucket }
output "url" { value = "https://${var.domain}" }
