#!/usr/bin/env ruby
# frozen_string_literal: true

require "json"
require_relative "release-policy"

roles = ReleasePolicy::ROLE_ENV.each_with_index.to_h do |name, index|
  [name, "ghcr.io/alpha/#{name.downcase.tr('_', '-')}@sha256:#{(index + 10).to_s(16) * 64}"]
end
services = ReleasePolicy::SERVICE_IMAGES.to_h { |service, name| [service, { "image" => roles.fetch(name), "environment" => {} }] }
services["judge-worker"]["environment"]["JUDGE_IMAGE"] = roles.fetch("JUDGE_IMAGE")
services["workspace-worker"]["environment"]["WORKSPACE_IMAGE"] = roles.fetch("WORKSPACE_IMAGE")
services["db"] = { "image" => "postgres@sha256:#{'9' * 64}" }
config = { "services" => services }

def reject(label)
  begin
    yield
  rescue RuntimeError
    return
  end
  raise "#{label} unexpectedly passed"
end

raise unless ReleasePolicy.required_images(config, roles).length == 6
reject("missing role") { ReleasePolicy.required_images(config, roles.reject { |name| name == "ALPHA_WEB_IMAGE" }) }
reject("extra list") { ReleasePolicy.required_images(config, roles.merge("RELEASE_IMAGE_REFS" => roles.values.first)) }
reject("duplicate role") { ReleasePolicy.required_images(config, roles.merge("ALPHA_WEB_IMAGE" => roles.fetch("ALPHA_API_IMAGE"))) }

image = roles.fetch("ALPHA_API_IMAGE")
repository, digest = ReleasePolicy.image_parts(image)
signer = "release@alpha.example"
signature = [{ "critical" => { "identity" => { "docker-reference" => repository }, "image" => { "docker-manifest-digest" => "sha256:#{digest}" } }, "optional" => { "signer" => signer } }]
ReleasePolicy.verify_signature(signature, image, signer)

source = "https://github.com/example/alpha"
commit = "1" * 40
builder = "https://github.com/actions/runner"
provenance = {
  "predicateType" => "https://slsa.dev/provenance/v1",
  "subject" => [{ "name" => repository, "digest" => { "sha256" => digest } }],
  "predicate" => {
    "buildDefinition" => {
      "externalParameters" => { "source" => { "repository" => source, "revision" => commit } },
      "resolvedDependencies" => [{ "uri" => source, "digest" => { "gitCommit" => commit } }]
    },
    "runDetails" => { "builder" => { "id" => builder } }
  }
}
ReleasePolicy.verify_provenance([provenance], image, source, commit, [builder])
reject("wrong source") { ReleasePolicy.verify_provenance([provenance], image, "https://github.com/evil/repo", commit, [builder]) }
reject("wrong commit") { ReleasePolicy.verify_provenance([provenance], image, source, "2" * 40, [builder]) }
reject("wrong builder") { ReleasePolicy.verify_provenance([provenance], image, source, commit, ["https://evil.example/builder"]) }

sbom = {
  "predicateType" => "https://spdx.dev/Document",
  "subject" => [{ "name" => repository, "digest" => { "sha256" => digest } }],
  "predicate" => { "packages" => [{ "name" => "alpha" }] }
}
ReleasePolicy.verify_sbom([sbom], image)
reject("empty packages") { ReleasePolicy.verify_sbom([sbom.merge("predicate" => { "packages" => [] })], image) }

puts "release policy smoke passed"
