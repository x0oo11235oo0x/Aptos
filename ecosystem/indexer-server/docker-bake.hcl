# This is a docker bake file in HCL syntax.
# It provides a high-level mechenanism to build multiple dockerfiles in one shot.
# Check https://crazymax.dev/docker-allhands2-buildx-bake and https://docs.docker.com/engine/reference/commandline/buildx_bake/#file-definition for an intro.


variable "GIT_SHA" {}
variable "TARGET_CACHE_ID" {}
variable "AWS_ECR_ACCOUNT_NUM" {}
variable "GCP_DOCKER_ARTIFACT_REPO" {}
variable "ecr_base" {
  default = "${AWS_ECR_ACCOUNT_NUM}.dkr.ecr.us-west-2.amazonaws.com/aptos"
}
variable "normalized_target_cache_id" {
  default = regex_replace("${TARGET_CACHE_ID}", "[^a-zA-Z0-9]", "-")
}

group "default" {
  targets = [
    "indexer-server",
  ]
}

target "indexer-server" {
  dockerfile = "Dockerfile"
  context    = "."
  cache-from = [
    "type=registry,ref=${GCP_DOCKER_ARTIFACT_REPO}/indexer-server:cache-main",
    "type=registry,ref=${GCP_DOCKER_ARTIFACT_REPO}/indexer-server:cache-auto",
    "type=registry,ref=${GCP_DOCKER_ARTIFACT_REPO}/indexer-server:cache-${normalized_target_cache_id}",
  ]
  cache-to = ["type=registry,ref=${GCP_DOCKER_ARTIFACT_REPO}/indexer-server:cache-${normalized_target_cache_id},mode=max"]
  tags = [
    "${ecr_base}/indexer-server:${GIT_SHA}",
    "${GCP_DOCKER_ARTIFACT_REPO}/indexer-server:${GIT_SHA}",
  ]
}
