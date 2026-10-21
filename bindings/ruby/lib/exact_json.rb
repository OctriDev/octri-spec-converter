# frozen_string_literal: true

require 'json'
require 'bigdecimal'

module OctriExactJSON
  def self.parse(text)
    JSON.parse(text, decimal_class: BigDecimal, max_nesting: 256)
  end

  def self.generate(value)
    chunks = []
    active = {}
    size = 0
    nodes = 0
    append = lambda do |token|
      size += token.bytesize
      raise ArgumentError, 'JSON transport exceeds 128 MiB' if size > 128 * 1024 * 1024
      chunks << token
    end
    write = lambda do |item, depth|
      nodes += 1
      raise ArgumentError, 'JSON transport exceeds nesting or occurrence limit' if depth > 256 || nodes > 4_000_000
      case item
      when BigDecimal
        raise ArgumentError, 'JSON decimals must be finite' unless item.finite?
        append.call(item.to_s)
      when Hash, Array
        raise ArgumentError, 'Circular JSON transport' if active[item.object_id]
        active[item.object_id] = true
        mapping = item.is_a?(Hash)
        append.call(mapping ? '{' : '[')
        item.each_with_index do |entry, index|
          append.call(',') unless index.zero?
          if mapping
            key, child = entry
            raise TypeError, 'JSON object keys must be strings' unless key.is_a?(String)
            append.call(JSON.generate(key) + ':')
          else
            child = entry
          end
          write.call(child, depth + 1)
        end
        append.call(mapping ? '}' : ']')
        active.delete(item.object_id)
      when String, Integer, Float, TrueClass, FalseClass, NilClass
        append.call(JSON.generate(item))
      else
        raise TypeError, 'JSON transport accepts plain containers and numeric values'
      end
    end
    write.call(value, 0)
    chunks.join
  end
end
