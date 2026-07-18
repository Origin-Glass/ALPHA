#!/usr/bin/env ruby

require "yaml"

spec = YAML.safe_load(File.read("openapi.yaml"), [], [], true)
documented = spec.fetch("paths").keys.sort
implemented = File.read("src/http.rs").scan(/\.route\(\s*"([^"]+)"/m).flatten.sort

missing = implemented - documented
extra = documented - implemented
abort "OpenAPI 누락: #{missing.join(", ")}" unless missing.empty?
abort "구현 없는 OpenAPI 경로: #{extra.join(", ")}" unless extra.empty?

puts "OpenAPI #{documented.length}개 경로 일치"
