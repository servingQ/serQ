import Serq.Claims
open SerqLang SerqLang.Exec
def W : Workload := ⟨[[(9, 1900), (10, 400)], [(9, 100), (10, 20)], [(9, 1500), (10, 300)], [(9, 50), (10, 0)], [(9, 800), (10, 350)], [(9, 1999), (10, 499)], [(9, 1000), (10, 100)], [(9, 1200), (10, 200)], [(9, 1700), (10, 450)], [(9, 300), (10, 30)], [(9, 1999), (10, 499)], [(9, 1999), (10, 499)], [(9, 1999), (10, 499)], [(9, 1999), (10, 499)], [(9, 1999), (10, 499)], [(9, 1999), (10, 499)], [(9, 1999), (10, 499)], [(9, 1999), (10, 499)]], [], none, 0, some 8⟩
def D := Claims.KongSvf.deployment
def m := Exec.runW D 100000 W Claims.KongSvf.prog
#eval (m.now, (m.sess.toList.map (·.status)).all (· == .ended), observed m 2, Exec.total m 2, Exec.total m 0, Exec.prefixTotal (Exec.values m 1), Exec.total m 1)
#eval 17500 * (Exec.total m 2 - Exec.total m 0) ≤ 2 * (Exec.prefixTotal (Exec.values m 1) - Exec.total m 1)
