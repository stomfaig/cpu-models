addi x1, x0, 5
addi x2, x0, 0
loop:
add  x2, x2, x1
addi x1, x1, -1
bne  x1, x0, loop