arr = [5, 2, 8, 1, 9, 2]
puts arr.sort.uniq.inspect
puts arr.map { |x| x * 2 }.inspect
puts arr.select { |x| x > 3 }.inspect
puts arr.reduce(0) { |acc, x| acc + x }
puts arr.sum
puts arr.min
puts arr.max
puts arr.any? { |x| x > 8 }
puts arr.all? { |x| x > 0 }
puts arr.join("-")

h = { a: 1, b: 2 }
h[:c] = 3
puts h[:b]
puts h.keys.length
puts h.has_key?(:c)
h.delete(:a)
puts h.size

s = "  hello world  "
puts s.strip.upcase
puts "a,b,c".split(",").reverse.join(":")
puts "hello".gsub("l", "x")
puts "42".to_i + 8
puts((1..5).to_a.inspect)
puts((1...5).include?(5))
