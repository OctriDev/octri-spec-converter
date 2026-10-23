Gem::Specification.new do |spec|
  spec.name = 'octri-spec-converter'
  spec.version = '0.1.3'
  spec.summary = 'Bindings for the offline Octri Rust Spec Converter'
  spec.authors = ['Octri contributors']
  spec.homepage = 'https://github.com/octridev/octri-spec-converter'
  spec.description = 'Convert API descriptions to OpenAPI offline with the bundled Octri Rust Spec Converter.'
  spec.files = ['lib/octri_converter.rb', 'lib/exact_json.rb', 'LICENSE', 'OAI-LICENSE', 'THIRD-PARTY-NOTICES.md',
                *Dir['lib/octri_converter/native/*']]
  spec.required_ruby_version = '>= 3.1'
  spec.license = 'Apache-2.0'
  # Bundled rather than default gems from Ruby 3.4 (bigdecimal) and 4.0 (fiddle).
  spec.add_dependency 'bigdecimal'
  spec.add_dependency 'fiddle'
end
