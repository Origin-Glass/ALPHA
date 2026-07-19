#!/usr/bin/env ruby
# frozen_string_literal: true

require "base64"
require "json"

module ReleasePolicy
  ROLE_ENV = %w[
    ALPHA_API_IMAGE ALPHA_WEB_IMAGE ALPHA_JUDGE_WORKER_IMAGE
    ALPHA_WORKSPACE_WORKER_IMAGE JUDGE_IMAGE WORKSPACE_IMAGE
  ].freeze
  SERVICE_IMAGES = {
    "migrate" => "ALPHA_API_IMAGE",
    "publication-gate" => "ALPHA_API_IMAGE",
    "api" => "ALPHA_API_IMAGE",
    "metadata-worker" => "ALPHA_API_IMAGE",
    "content-worker" => "ALPHA_API_IMAGE",
    "web" => "ALPHA_WEB_IMAGE",
    "judge-worker" => "ALPHA_JUDGE_WORKER_IMAGE",
    "workspace-worker" => "ALPHA_WORKSPACE_WORKER_IMAGE",
    "judge-image" => "JUDGE_IMAGE"
  }.freeze
  EXPECTED_SERVICES = (SERVICE_IMAGES.keys + ["db"]).sort.freeze

  module_function

  def required_images(config, env)
    raise "RELEASE_IMAGE_REFS is not allowed; use required named image inputs" unless env.fetch("RELEASE_IMAGE_REFS", "").empty?

    images = ROLE_ENV.to_h do |name|
      image = env.fetch(name) { raise "#{name} is required" }
      raise "#{name} must use repository@sha256" unless image.match?(/@sha256:[0-9a-f]{64}\z/)
      raise "#{name} uses forbidden zero digest" if image.end_with?("@sha256:#{'0' * 64}")
      [name, image]
    end
    raise "distinct image roles must not use duplicate references" unless images.values.uniq.length == images.length

    services = config.fetch("services")
    raise "compose service image set changed; update release policy" unless services.keys.sort == EXPECTED_SERVICES
    SERVICE_IMAGES.each do |service, name|
      actual = services.fetch(service).fetch("image")
      raise "#{service} image does not match #{name}" unless actual == images.fetch(name)
    end
    raise "judge-worker JUDGE_IMAGE does not match" unless services.dig("judge-worker", "environment", "JUDGE_IMAGE") == images.fetch("JUDGE_IMAGE")
    raise "workspace-worker WORKSPACE_IMAGE does not match" unless services.dig("workspace-worker", "environment", "WORKSPACE_IMAGE") == images.fetch("WORKSPACE_IMAGE")

    images.values
  end

  def parse_entries(text)
    parsed = JSON.parse(text)
    parsed.is_a?(Array) ? parsed : [parsed]
  rescue JSON::ParserError
    text.lines.filter_map do |line|
      next if line.strip.empty?
      JSON.parse(line)
    end
  end

  def statement_entries(text)
    parse_entries(text).map do |entry|
      raise "attestation payload is missing" unless entry["payload"].is_a?(String)
      if entry["payloadType"] && entry["payloadType"] != "application/vnd.in-toto+json"
        raise "unexpected attestation payload type"
      end
      JSON.parse(Base64.strict_decode64(entry.fetch("payload")))
    end
  end

  def image_parts(image)
    match = image.match(/\A(.+)@sha256:([0-9a-f]{64})\z/) or raise "invalid image digest reference"
    [match[1], match[2]]
  end

  def verify_signature(entries, image, signer)
    repository, digest = image_parts(image)
    valid = entries.any? do |entry|
      entry.dig("critical", "identity", "docker-reference") == repository &&
        entry.dig("critical", "image", "docker-manifest-digest") == "sha256:#{digest}" &&
        entry.dig("optional", "signer") == signer
    end
    raise "signature identity or image digest does not match policy" unless valid
  end

  def subject_matches?(statement, repository, digest)
    statement.fetch("subject", []).any? do |subject|
      subject["name"] == repository && subject.dig("digest", "sha256") == digest
    end
  end

  def verify_provenance(statements, image, source_repository, commit, builders)
    repository, digest = image_parts(image)
    valid = statements.any? do |statement|
      source = statement.dig("predicate", "buildDefinition", "externalParameters", "source")
      dependencies = statement.dig("predicate", "buildDefinition", "resolvedDependencies") || []
      statement["predicateType"] == "https://slsa.dev/provenance/v1" &&
        subject_matches?(statement, repository, digest) &&
        source == { "repository" => source_repository, "revision" => commit } &&
        builders.include?(statement.dig("predicate", "runDetails", "builder", "id")) &&
        dependencies.any? { |material| material["uri"] == source_repository && material.dig("digest", "gitCommit") == commit }
    end
    raise "provenance source, commit material, builder, predicate, or subject does not match policy" unless valid
  end

  def verify_sbom(statements, image)
    repository, digest = image_parts(image)
    valid = statements.any? do |statement|
      packages = statement.dig("predicate", "packages")
      statement["predicateType"] == "https://spdx.dev/Document" &&
        subject_matches?(statement, repository, digest) &&
        packages.is_a?(Array) && !packages.empty? && packages.all? { |package| !package["name"].to_s.empty? }
    end
    raise "SPDX predicate, subject, or package inventory does not match policy" unless valid
  end
end

if $PROGRAM_NAME == __FILE__
  mode, image, path = ARGV
  case mode
  when "images"
    config = JSON.parse(File.read(image))
    puts ReleasePolicy.required_images(config, ENV)
  when "signature"
    ReleasePolicy.verify_signature(ReleasePolicy.parse_entries(File.read(path)), image, ENV.fetch("EXPECTED_SIGNER_IDENTITY"))
  when "provenance"
    commit = ENV.fetch("EXPECTED_COMMIT")
    raise "EXPECTED_COMMIT must be a full Git digest" unless commit.match?(/\A[0-9a-f]{40}(?:[0-9a-f]{24})?\z/)
    builders = ENV.fetch("APPROVED_BUILDERS").split(",").map(&:strip).reject(&:empty?)
    raise "APPROVED_BUILDERS is empty" if builders.empty?
    ReleasePolicy.verify_provenance(ReleasePolicy.statement_entries(File.read(path)), image, ENV.fetch("EXPECTED_SOURCE_REPOSITORY"), commit, builders)
  when "sbom"
    ReleasePolicy.verify_sbom(ReleasePolicy.statement_entries(File.read(path)), image)
  else
    raise "usage: #{$PROGRAM_NAME} images COMPOSE.json | signature|provenance|sbom IMAGE RESULT.json"
  end
end
