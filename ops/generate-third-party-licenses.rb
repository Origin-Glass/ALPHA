#!/usr/bin/env ruby
# frozen_string_literal: true

require "digest"
require "json"
require "open3"
require "pathname"

ROOT = Pathname(__dir__).join("..").expand_path
OUTPUT = Pathname(ENV.fetch("THIRD_PARTY_LICENSE_OUTPUT", ROOT.join("THIRD_PARTY_LICENSES.txt").to_s))
CHECKSUM = Pathname("#{OUTPUT}.sha256")
LICENSE_FILE = /\A(?:LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)(?:[._-].*)?\z/i
COMMON_LICENSES = {
  "Apache-2.0" => "Apache-2.0",
  "CC0-1.0" => "CC0-1.0",
  "GPL-1.0-only" => "GPL-1",
  "GPL-1.0-or-later" => "GPL-1",
  "GPL-2.0-only" => "GPL-2",
  "GPL-2.0-or-later" => "GPL-2",
  "GPL-3.0-only" => "GPL-3",
  "GPL-3.0-or-later" => "GPL-3",
  "LGPL-2.0-only" => "LGPL-2",
  "LGPL-2.0-or-later" => "LGPL-2",
  "LGPL-2.1-only" => "LGPL-2.1",
  "LGPL-2.1-or-later" => "LGPL-2.1",
  "LGPL-3.0-only" => "LGPL-3",
  "LGPL-3.0-or-later" => "LGPL-3",
  "MPL-2.0" => "MPL-2.0"
}.freeze
EMBEDDED_LICENSES = {
  "0BSD" => <<~TEXT
    Permission to use, copy, modify, and/or distribute this software for any
    purpose with or without fee is hereby granted.

    THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
    WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
    MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
    SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
    WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
    OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
    CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
  TEXT
}.freeze

def text_files(directory)
  return [] unless directory.directory?

  directory.children.select { |path| path.file? && path.basename.to_s.match?(LICENSE_FILE) }.sort.map do |path|
    text = path.binread.encode("UTF-8", invalid: :replace, undef: :replace, replace: "�").gsub("\r\n", "\n")
    text = "#{text}\n" unless text.end_with?("\n")
    [path.basename.to_s, text]
  end
end

def author_strings(value)
  case value
  when String then [value]
  when Array then value.filter_map { |entry| entry.is_a?(String) ? entry : entry["name"] }
  when Hash then [value["name"]].compact
  else []
  end
end

def spdx_ids(expression)
  expression.to_s.scan(/[A-Za-z0-9][A-Za-z0-9.+-]*/).reject { |token| %w[AND OR WITH].include?(token) }
end

stdout, stderr, status = Open3.capture3("cargo", "metadata", "--locked", "--format-version", "1", chdir: ROOT.to_s)
abort stderr unless status.success?
cargo = JSON.parse(stdout)
records = cargo.fetch("packages").filter_map do |package|
  next unless package["source"]

  directory = Pathname(package.fetch("manifest_path")).dirname
  files = text_files(directory)
  {
    ecosystem: "cargo",
    name: package.fetch("name"),
    version: package.fetch("version"),
    source: package.fetch("source"),
    license: package["license"] || (package["license_file"] && "SEE-LICENSE-FILE"),
    authors: package.fetch("authors", []),
    texts: files.map(&:last)
  }
end

lock = JSON.parse(ROOT.join("web/package-lock.json").read)
lock.fetch("packages").each do |path, metadata|
  next if path.empty?

  directory = ROOT.join("web", path)
  package_json = directory.join("package.json")
  package = package_json.file? ? JSON.parse(package_json.read) : {}
  records << {
    ecosystem: "npm",
    name: package["name"] || path.sub(%r{.*node_modules/}, ""),
    version: metadata.fetch("version"),
    source: metadata.fetch("resolved"),
    license: metadata["license"] || package["license"],
    authors: author_strings(package["author"]),
    texts: text_files(directory).map(&:last)
  }
end

records.uniq! { |record| [record[:ecosystem], record[:name], record[:version], record[:source]] }
shared_texts = records.each_with_object(Hash.new { |hash, key| hash[key] = [] }) do |record, index|
  spdx_ids(record[:license]).each { |identifier| index[identifier].concat(record[:texts]) }
end
common_directory = Pathname(ENV.fetch("COMMON_LICENSE_DIR", "/usr/share/common-licenses"))
COMMON_LICENSES.each do |identifier, filename|
  path = common_directory.join(filename)
  next unless path.file?
  text = path.binread.encode("UTF-8", invalid: :replace, undef: :replace, replace: "�").gsub("\r\n", "\n")
  text = "#{text}\n" unless text.end_with?("\n")
  shared_texts[identifier] << text
end
EMBEDDED_LICENSES.each { |identifier, text| shared_texts[identifier] << text }
records.each do |record|
  abort "missing declared license: #{record[:ecosystem]}:#{record[:name]}@#{record[:version]}" if record[:license].to_s.empty?
  record[:texts] = spdx_ids(record[:license]).flat_map { |identifier| shared_texts[identifier] }.uniq if record[:texts].empty?
  abort "missing license text: #{record[:ecosystem]}:#{record[:name]}@#{record[:version]} (#{record[:license]})" if record[:texts].empty?
end

texts = records.flat_map { |record| record[:texts] }.uniq.to_h { |text| [Digest::SHA256.hexdigest(text), text] }
records.sort_by! { |record| [record[:ecosystem], record[:name].downcase, record[:version], record[:source]] }

lines = [
  "ALPHA deterministic third-party attribution and license bundle",
  "format-version: 1",
  "Cargo.lock-sha256: #{Digest::SHA256.file(ROOT.join('Cargo.lock')).hexdigest}",
  "package-lock.json-sha256: #{Digest::SHA256.file(ROOT.join('web/package-lock.json')).hexdigest}",
  "packages: #{records.length}",
  "license-texts: #{texts.length}",
  "",
  "PACKAGE ATTRIBUTION"
]
records.each do |record|
  own_copyright = record[:texts].flat_map do |text|
    text.lines.map(&:strip).select { |line| line.match?(/copyright|©/i) }
  end.uniq.sort
  lines << ""
  lines << "#{record[:ecosystem]}:#{record[:name]}@#{record[:version]}"
  lines << "Source: #{record[:source]}"
  lines << "Declared-License: #{record[:license]}"
  record[:authors].map(&:strip).reject(&:empty?).uniq.sort.each { |author| lines << "Author: #{author}" }
  own_copyright.each { |copyright| lines << "Copyright: #{copyright}" }
  record[:texts].map { |text| Digest::SHA256.hexdigest(text) }.uniq.sort.each { |digest| lines << "License-Text-SHA256: #{digest}" }
end

lines << ""
lines << "LICENSE TEXTS"
texts.sort.each do |digest, text|
  lines << ""
  lines << "===== SHA256 #{digest} ====="
  lines << text.chomp
end
rendered = "#{lines.join("\n")}\n"
checksum = "#{Digest::SHA256.hexdigest(rendered)}  THIRD_PARTY_LICENSES.txt\n"

if ARGV == ["--check"]
  actual_digest = OUTPUT.file? ? Digest::SHA256.file(OUTPUT).hexdigest : "missing"
  expected_digest = Digest::SHA256.hexdigest(rendered)
  abort "THIRD_PARTY_LICENSES.txt is stale: #{actual_digest} != #{expected_digest}" unless actual_digest == expected_digest
  abort "THIRD_PARTY_LICENSES.txt.sha256 is stale" unless CHECKSUM.file? && CHECKSUM.read == checksum
  puts "third-party attribution bundle is current (#{records.length} packages, #{texts.length} license texts)"
else
  temporary = OUTPUT.sub_ext(".tmp")
  temporary.write(rendered)
  temporary.rename(OUTPUT)
  checksum_temporary = CHECKSUM.sub_ext(".tmp")
  checksum_temporary.write(checksum)
  checksum_temporary.rename(CHECKSUM)
  puts "generated #{OUTPUT.basename} (#{records.length} packages, #{texts.length} license texts)"
end
