require 'json'
require_relative 'lib/octri_converter'
converter = OctriConverter::Converter.new(ARGV.fetch(0))
input = OctriExactJSON.parse(STDIN.read)
action = input['action']
result = if ENV['OCTRI_BINDING_RAW'] == '1' || input['abiVersion'] != 1 || !%w[convert review bundle compare export formats detect batch].include?(action)
  converter.call(input)
else
  value = case action
  when 'convert', 'review', 'bundle' then converter.public_send(action, input.fetch('request'))
  when 'compare' then converter.compare(input.fetch('before'), input.fetch('after'))
  when 'export' then converter.export(input.fetch('report'), input.fetch('format'))
  when 'detect' then converter.detect(input.fetch('request'))
  when 'batch' then converter.batch(input.fetch('requests'))
  when 'formats' then converter.formats
  end
  { 'Ok' => value }
end
puts OctriExactJSON.generate(result)
