# trust-sandbox hosting VM — a small, self-contained cloud instance that runs
# the (isolated) trust-sandbox stack and joins the ZAGROS mesh. Deliberately NOT
# part of the shared terraform/ tree: its own state key so it can be wiped
# independently (the sandbox is ephemeral by design — see deploy/README.md).
#
# Account: legacy (123456789013) · region eu-central-1 (near the ZAGROS rla relay).
# Apply:   aws-vault exec legacy --no-session -- terraform -chdir=ops/serve/trust-sandbox/deploy/terraform apply
#
# The VM self-hosts its TLS+Ory gate; ZAGROS is touched only additively (DNS, an
# Ory client, an isitup row). Cloud chosen over the Thor to avoid leeching it
# (docs/design/trust-sandbox.md §Delivery). The Thor-local path is wired but not
# built — see deploy/README.md §"Local-on-Thor (stub)".

terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = { source = "hashicorp/aws", version = "~> 5.0" }
  }
  backend "s3" {
    bucket = "example-legacy-tf-state"
    key    = "trust-sandbox/terraform.tfstate"
    region = "eu-central-1"
  }
}

variable "aws_account_id" {
  type        = string
  description = <<-EOT
    AWS account this sandbox is applied into. No default — the allowed_account_ids
    fuse must refuse to apply rather than follow whatever account the ambient
    aws-vault profile points at (ADR 0219; F-271 is what the missing fuse costs).
    The header above records legacy (123456789013) as the account the existing
    instance and state key live in; that estate is teardown-only, so a fresh apply
    should name the usareur-af account and move the state key with it.
  EOT
}

provider "aws" {
  region              = "eu-central-1"
  allowed_account_ids = [var.aws_account_id]
}

variable "operator_cidr" {
  description = "Your public IP /32 for SSH (deploy-time only; the service itself is mesh-only)."
  type        = string
}
variable "public_key" {
  description = "SSH public key for the deploy user."
  type        = string
}
variable "instance_type" {
  type    = string
  default = "t4g.small" # 2 vCPU / 2 GB ARM (images are multi-arch); ~$12/mo
}

data "aws_vpc" "default" { default = true }
data "aws_subnets" "default" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default.id]
  }
}
data "aws_ami" "ubuntu_arm" {
  most_recent = true
  owners      = ["099720109477"] # Canonical
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-arm64-server-*"]
  }
}

resource "aws_key_pair" "deploy" {
  key_name   = "trust-sandbox-deploy"
  public_key = var.public_key
}

resource "aws_security_group" "ts" {
  name        = "trust-sandbox"
  description = "trust-sandbox VM: egress all (NetBird relay/P2P + ghcr pull); ingress SSH from operator only. Service is mesh-only via wt0, not the public ENI."
  vpc_id      = data.aws_vpc.default.id
  ingress {
    description = "SSH (deploy-time)"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = [var.operator_cidr]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
  tags = { Name = "trust-sandbox", project = "trust-sandbox", ephemeral = "true" }
}

resource "aws_instance" "ts" {
  ami                         = data.aws_ami.ubuntu_arm.id
  instance_type               = var.instance_type
  subnet_id                   = tolist(data.aws_subnets.default.ids)[0]
  vpc_security_group_ids      = [aws_security_group.ts.id]
  key_name                    = aws_key_pair.deploy.key_name
  associate_public_ip_address = true
  root_block_device {
    volume_size = 20
    volume_type = "gp3"
  }
  user_data = <<-EOF
    #!/bin/bash
    set -eux
    apt-get update
    apt-get install -y ca-certificates curl gnupg
    install -m0755 -d /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
    chmod a+r /etc/apt/keyrings/docker.asc
    echo "deb [arch=arm64 signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu noble stable" > /etc/apt/sources.list.d/docker.list
    apt-get update
    apt-get install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin
    usermod -aG docker ubuntu
    # NetBird client (enrolled by the deploy step with the ZAGROS setup key)
    curl -fsSL https://pkgs.netbird.io/install.sh | sh
  EOF
  tags = { Name = "trust-sandbox", project = "trust-sandbox", ephemeral = "true" }
}

output "public_ip" { value = aws_instance.ts.public_ip }
output "instance_id" { value = aws_instance.ts.id }
output "ssh" { value = "ssh ubuntu@${aws_instance.ts.public_ip}" }
