#!/usr/bin/env ruby

require "psych"

source = File.read("openapi.yaml")
Psych.parse_stream(source)
contract = Psych.safe_load(source, aliases: true)
documented = source.scan(/^  (\/[^:]+):$/).flatten.sort
implemented = File.read("src/http.rs").scan(/\.route\(\s*"([^"]+)"/m).flatten.sort

missing = implemented - documented
extra = documented - implemented
abort "OpenAPI 누락: #{missing.join(", ")}" unless missing.empty?
abort "구현 없는 OpenAPI 경로: #{extra.join(", ")}" unless extra.empty?

operations_schema = contract.dig("paths", "/api/v1/admin/operations", "get", "responses", "200", "content", "application/json", "schema", "$ref")
abort "운영 진단 응답 스키마 누락" unless operations_schema == "#/components/schemas/OperationalSnapshot"
snapshot = contract.dig("components", "schemas", "OperationalSnapshot")
abort "운영 진단 필수 범주 누락" unless snapshot["required"] == %w[providers generation_jobs reviews rights learning workspaces]
abort "운영 조치 항목 상한 누락" unless snapshot["properties"].values.all? { |value| value.dig("properties", "action_items", "maxItems") == 20 }

puts "OpenAPI #{documented.length}개 경로 일치"
