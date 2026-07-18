#!/usr/bin/env ruby

require "psych"

source = File.read("openapi.yaml")
Psych.parse_stream(source)
documented = source.scan(/^  (\/[^:]+):$/).flatten.sort
implemented = File.read("src/http.rs").scan(/\.route\(\s*"([^"]+)"/m).flatten.sort

missing = implemented - documented
extra = documented - implemented
abort "OpenAPI 누락: #{missing.join(", ")}" unless missing.empty?
abort "구현 없는 OpenAPI 경로: #{extra.join(", ")}" unless extra.empty?

puts "OpenAPI #{documented.length}개 경로 일치"
