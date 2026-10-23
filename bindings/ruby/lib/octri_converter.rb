# frozen_string_literal: true

require 'fiddle'
require 'json'
require_relative 'exact_json'

module OctriConverter
  class ConversionError < StandardError
    attr_reader :code, :diagnostics
    def initialize(error)
      super(error.fetch('message'))
      @code = error.fetch('code')
      @diagnostics = error.fetch('diagnostics', [])
    end
  end

  class Converter
    # Platform gems carry the native library under lib/octri_converter/native.
    BUNDLED_LIBRARY = File.join(__dir__, 'octri_converter', 'native',
                                RUBY_PLATFORM.include?('darwin') ? 'liboctri_converter_ffi.dylib' : 'liboctri_converter_ffi.so')

    def initialize(library = BUNDLED_LIBRARY)
      @library = Fiddle.dlopen(File.realpath(library))
      @version = function('abi_version', [], Fiddle::TYPE_INT)
      raise 'Expected converter ABI version 1' unless @version.call == 1
      @call = function('call', [Fiddle::TYPE_VOIDP, Fiddle::TYPE_SIZE_T], Fiddle::TYPE_VOIDP)
      @data = function('buffer_data', [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOIDP)
      @length = function('buffer_len', [Fiddle::TYPE_VOIDP], Fiddle::TYPE_SIZE_T)
      @free = function('buffer_free', [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)
    end

    def call(envelope)
      input = OctriExactJSON.generate(envelope).encode(Encoding::UTF_8)
      raise ArgumentError, 'Converter envelope exceeds 128 MiB' if input.bytesize > 128 * 1024 * 1024
      output = @call.call(Fiddle::Pointer[input], input.bytesize)
      raise NoMemoryError, 'Native converter returned a null buffer' if output.null?
      begin
        text = @data.call(output).to_s(@length.call(output)).force_encoding(Encoding::UTF_8)
      ensure
        @free.call(output)
      end
      OctriExactJSON.parse(text)
    end

    def convert(request) = result('convert', 'request' => request)
    def review(request) = result('review', 'request' => request)
    def bundle(request) = result('bundle', 'request' => request)
    def compare(before, after) = result('compare', 'before' => before, 'after' => after)
    def export(report, format = 'json') = result('export', 'report' => report, 'format' => format)
    def formats = result('formats')
    def detect(request) = result('detect', 'request' => request)
    def batch(requests) = result('batch', 'requests' => requests)

    private

    def function(name, args, output)
      Fiddle::Function.new(@library["octri_converter_#{name}"], args, output)
    end

    def result(action, fields = {})
      response = call({ 'abiVersion' => 1, 'action' => action }.merge(fields))
      raise ConversionError, response['Err'] if response.key?('Err')
      response.fetch('Ok')
    end
  end

  def self.text(content, uri = 'memory://convert/openapi.yaml')
    { 'uri' => uri, 'content' => content, 'generated' => false }
  end
end
