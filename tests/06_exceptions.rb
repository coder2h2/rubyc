begin
  x = 10 / 0
  puts x
rescue ZeroDivisionError => e
  puts "caught zero div: #{e.message}"
ensure
  puts "ensure 1"
end

begin
  raise "boom!"
rescue => e
  puts "caught: #{e.message}"
end

a, b = 10, 20
puts a + b

obj = nil
puts obj&.upcase.nil?
