ui = false
disable_mlock = false

storage "raft" {
  path    = "/vault/data"
  node_id = "chaos-vault-1"
}

listener "tcp" {
  address            = "0.0.0.0:8200"
  cluster_address    = "0.0.0.0:8201"
  tls_cert_file      = "/vault/tls/vault.pem"
  tls_key_file       = "/vault/tls/vault-key.pem"
  tls_client_ca_file = "/vault/tls/ca.pem"
  tls_min_version    = "tls13"
}

api_addr     = "https://vault:8200"
cluster_addr = "https://vault:8201"

