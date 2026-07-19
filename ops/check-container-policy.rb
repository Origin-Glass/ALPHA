#!/usr/bin/env ruby
# frozen_string_literal: true

approved = ENV.fetch("APPROVED_IMAGE_REGISTRIES", "docker.io,ghcr.io").split(",").map(&:strip)
errors = []

Dir.glob("**/Dockerfile", File::FNM_DOTMATCH).sort.each do |path|
  stages = []
  File.foreach(path).with_index(1) do |line, number|
    match = line.match(/^\s*FROM\s+(?:(?:--\S+)\s+)*(\S+)(?:\s+AS\s+(\S+))?/i)
    next unless match

    image = match[1]
    if stages.any? { |stage| stage.casecmp?(image) }
      stages << match[2] if match[2]
      next
    end

    registry = image.include?("/") ? image.split("/", 2).first : "docker.io"
    registry = "docker.io" unless registry.include?(".") || registry.include?(":") || registry == "localhost"
    errors << "#{path}:#{number}: 승인되지 않은 registry: #{registry}" unless approved.include?(registry)
    errors << "#{path}:#{number}: digest로 고정되지 않은 외부 image: #{image}" unless image.match?(/@sha256:[0-9a-f]{64}\z/)
    stages << match[2] if match[2]
  end
end

abort errors.join("\n") unless errors.empty?
